//! Keeping the API key and secret out of everything printed.
//!
//! ICANotes takes both as query parameters, so any text that could carry a
//! request URL (an error, a passthrough's answer) passes through a
//! [`Redactor`] first. reqwest's own errors are taken `without_url`, so this
//! is the second line, not the only one.
//!
//! Two passes. The first replaces each value where it appears as written or
//! in a common encoding, keeping the text around it. The second catches
//! every other spelling: a token (a run between whitespace, quotes and
//! brackets) holding `%`, `\` or `&#` is decoded (percent-encoding in either
//! case, JSON escapes, HTML character references, nested up to three deep),
//! and replaced whole when the decoded form holds a value. So a value an
//! upstream echoed JSON-escaped (`ab\/cd\u002Bef`), partly or doubly
//! percent-encoded, or as HTML, never reaches the output. A value is ASCII
//! with no whitespace (`credential.rs`), so no value spans two tokens.

/// What a redacted value is replaced with.
pub const REDACTED: &str = "[redacted]";

/// Characters that end a token, unless a value contains them.
const DELIMITERS: &[u8] = b"\"'<>,(){}[]";
/// How many decodings deep the second pass looks (`%252B` is two).
const DEPTH: usize = 3;

/// Replaces each of its values, however encoded, with [`REDACTED`].
#[derive(Default, Clone)]
pub struct Redactor {
    values: Vec<Vec<u8>>,
    needles: Vec<Vec<u8>>,
    delimiters: Vec<u8>,
}

impl std::fmt::Debug for Redactor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Redactor({} values)", self.values.len())
    }
}

impl Redactor {
    pub fn new<'a>(values: impl IntoIterator<Item = &'a str>) -> Self {
        let values: Vec<Vec<u8>> = values
            .into_iter()
            .filter(|v| !v.is_empty())
            .map(|v| v.as_bytes().to_vec())
            .collect();
        let mut needles: Vec<Vec<u8>> = Vec::new();
        for v in &values {
            let form: String = url::form_urlencoded::byte_serialize(v).collect();
            let percent = form.replace('+', "%20");
            let every: String = v.iter().map(|b| format!("%{b:02X}")).collect();
            for n in [
                String::from_utf8_lossy(v).into_owned(),
                form.clone(),
                lower_hex(&form),
                percent.clone(),
                lower_hex(&percent),
                every.clone(),
                every.to_ascii_lowercase(),
            ] {
                let n = n.into_bytes();
                if !needles.contains(&n) {
                    needles.push(n);
                }
            }
        }
        // Longest first, so a value that contains another is replaced whole.
        needles.sort_by_key(|n| std::cmp::Reverse(n.len()));
        let delimiters = DELIMITERS
            .iter()
            .copied()
            .filter(|d| !values.iter().any(|v| v.contains(d)))
            .collect();
        Redactor {
            values,
            needles,
            delimiters,
        }
    }

    pub fn text(&self, s: &str) -> String {
        // Both passes replace ASCII with ASCII, whole tokens or whole
        // needles, so valid UTF-8 stays valid.
        String::from_utf8(self.bytes(s.as_bytes()))
            .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
    }

    /// Bytes, for an answer that may not be text.
    pub fn bytes(&self, b: &[u8]) -> Vec<u8> {
        if self.values.is_empty() {
            return b.to_vec();
        }
        let mut out = b.to_vec();
        for n in &self.needles {
            out = replace_all(&out, n);
        }
        self.tokens(&out)
    }

    /// The second pass: each token that decodes to something holding a
    /// value, replaced whole.
    fn tokens(&self, b: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(b.len());
        let mut start = 0;
        let ends = |c: u8| c.is_ascii_whitespace() || self.delimiters.contains(&c);
        for i in 0..=b.len() {
            if i < b.len() && !ends(b[i]) {
                continue;
            }
            let token = &b[start..i];
            if self.hidden_in(token) {
                out.extend_from_slice(REDACTED.as_bytes());
            } else {
                out.extend_from_slice(token);
            }
            if i < b.len() {
                out.push(b[i]);
            }
            start = i + 1;
        }
        out
    }

    fn hidden_in(&self, token: &[u8]) -> bool {
        let mut cur = token.to_vec();
        for _ in 0..DEPTH {
            if !(cur.contains(&b'%') || cur.contains(&b'\\') || find(&cur, b"&#").is_some()) {
                return false;
            }
            let next = html_decode(&json_unescape(&percent_decode(&cur)));
            let next2 = percent_decode(&json_unescape(&html_decode(&cur)));
            for form in [&next, &next2] {
                if self.values.iter().any(|v| find(form, v).is_some()) {
                    return true;
                }
            }
            if next == cur {
                return false;
            }
            cur = next;
        }
        false
    }

    pub fn error(&self, e: pk_cli_core::CliError) -> pk_cli_core::CliError {
        let message = e.message().to_string();
        let clean = self.text(&message);
        let remedy = e.remedy().map(|r| (r.to_string(), self.text(r)));
        if clean == message && remedy.as_ref().is_none_or(|(r, c)| r == c) {
            return e;
        }
        let base = match e.exit_code() {
            2 => pk_cli_core::CliError::Usage(clean),
            3 => pk_cli_core::CliError::Auth(clean),
            4 => pk_cli_core::CliError::NotFound(clean),
            5 => pk_cli_core::CliError::Upstream(clean),
            _ => pk_cli_core::CliError::Other(clean),
        };
        let base = match e.reason() {
            Some(r) => base.because(r),
            None => base,
        };
        match remedy {
            Some((_, r)) => base.with_remedy(r),
            None => base,
        }
    }
}

fn lower_hex(encoded: &str) -> String {
    let b = encoded.as_bytes();
    let mut out = String::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            out.push('%');
            out.push((b[i + 1] as char).to_ascii_lowercase());
            out.push((b[i + 2] as char).to_ascii_lowercase());
            i += 3;
        } else {
            out.push(b[i] as char);
            i += 1;
        }
    }
    out
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    memchr::memmem::find(hay, needle)
}

fn replace_all(b: &[u8], n: &[u8]) -> Vec<u8> {
    if n.is_empty() {
        return b.to_vec();
    }
    let mut out = Vec::with_capacity(b.len());
    let mut last = 0;
    for at in memchr::memmem::find_iter(b, n) {
        if at < last {
            continue;
        }
        out.extend_from_slice(&b[last..at]);
        out.extend_from_slice(REDACTED.as_bytes());
        last = at + n.len();
    }
    out.extend_from_slice(&b[last..]);
    out
}

fn hex(c: u8) -> Option<u8> {
    (c as char).to_digit(16).map(|d| d as u8)
}

/// `%XX` in either case; `+` stays `+` (a value holds no space).
fn percent_decode(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push(h << 4 | l);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// JSON's escapes: `\/`, `\\`, `\"`, `\uXXXX` (ASCII only: a value is ASCII).
fn json_unescape(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 1 < b.len() {
            match b[i + 1] {
                c @ (b'/' | b'\\' | b'"' | b'\'') => {
                    out.push(c);
                    i += 2;
                    continue;
                }
                b'u' if i + 5 < b.len() => {
                    let code = b[i + 2..i + 6]
                        .iter()
                        .try_fold(0u32, |acc, &c| hex(c).map(|d| acc << 4 | d as u32));
                    if let Some(c) = code.filter(|c| *c < 0x80) {
                        out.push(c as u8);
                        i += 6;
                        continue;
                    }
                }
                _ => {}
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// HTML's numeric character references (`&#43;`, `&#x2B;`), ASCII only.
fn html_decode(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i..].starts_with(b"&#") {
            let (radix, from) = match b.get(i + 2) {
                Some(b'x' | b'X') => (16, i + 3),
                _ => (10, i + 2),
            };
            let mut j = from;
            while j < b.len() && j - from < 6 && (b[j] as char).is_digit(radix) {
                j += 1;
            }
            if j > from && b.get(j) == Some(&b';') {
                let digits = std::str::from_utf8(&b[from..j]).unwrap_or("");
                if let Some(c) = u32::from_str_radix(digits, radix)
                    .ok()
                    .filter(|c| *c < 0x80)
                {
                    out.push(c as u8);
                    i = j + 1;
                    continue;
                }
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "key-one";
    const SECRET: &str = "ab/cd+ef==";

    fn r() -> Redactor {
        Redactor::new([KEY, SECRET])
    }

    fn clean(s: &str) -> bool {
        !s.contains(KEY) && !s.contains("ab/cd") && !s.contains("cd+ef") && !s.contains("ab\\/")
    }

    #[test]
    fn a_value_is_replaced_as_written_and_encoded() {
        let s = r()
            .text("https://h/API/X?userKey=key-one&userSecret=ab%2Fcd%2Bef%3D%3D and ab/cd+ef==");
        assert!(clean(&s), "{s}");
        assert_eq!(s.matches(REDACTED).count(), 3, "{s}");
        // An exact match keeps the text around it.
        assert!(
            s.starts_with("https://h/API/X?userKey=[redacted]&userSecret=[redacted]"),
            "{s}"
        );
    }

    #[test]
    fn every_other_spelling_is_caught_too() {
        for spelled in [
            // JSON-escaped, as System.Text.Json and others write it.
            r#"{"echo":"ab\/cd\u002Bef=="}"#,
            r#"{"echo":"x?userSecret=ab\/cd\u002bef\u003D\u003D&y"}"#,
            // Lowercase percent-encoding, as .NET's HttpUtility writes it.
            "userSecret=ab%2fcd%2bef%3d%3d",
            // Partly encoded.
            "userSecret=ab/cd%2Bef==",
            "userSecret=ab%2Fcd+ef==",
            // Doubly encoded.
            "userSecret=ab%252Fcd%252Bef%253D%253D",
            // HTML references.
            "<td>ab&#47;cd&#43;ef&#x3D;&#x3d;</td>",
            // The key, encoded every byte.
            "u=%6B%65%79%2D%6F%6E%65",
            "u=%6b%65%79-%6f%6e%65",
        ] {
            let s = r().text(spelled);
            assert!(clean(&s), "{spelled} -> {s}");
            assert!(s.contains(REDACTED), "{spelled} -> {s}");
        }
    }

    #[test]
    fn redacted_json_stays_redacted_when_pretty_printed() {
        let body = br#"{"Message":"bad request ab\/cd\u002Bef== for key-one"}"#;
        let red = r().bytes(body);
        let v: serde_json::Value = serde_json::from_slice(&red).unwrap();
        let pretty = serde_json::to_string_pretty(&v).unwrap();
        assert!(clean(&pretty), "{pretty}");
    }

    #[test]
    fn a_value_holding_a_delimiter_is_still_found() {
        let r = Redactor::new(["k\"ey(1)"]);
        let s = r.text(r#"x=k%22ey%281%29 and "k\"ey(1)""#);
        assert!(!s.contains("ey(1"), "{s}");
        assert!(!s.contains("ey%28"), "{s}");
    }

    #[test]
    fn other_text_is_left_alone() {
        let s = r().text("a 50% chance, C:\\path, &#43; and ab/cx");
        assert_eq!(s, "a 50% chance, C:\\path, &#43; and ab/cx");
    }

    #[test]
    fn bytes_are_redacted_too() {
        let r = Redactor::new(["abc"]);
        assert_eq!(r.bytes(b"xabcyabc"), b"x[redacted]y[redacted]".to_vec());
        assert_eq!(r.bytes(b"ab"), b"ab".to_vec());
        assert_eq!(r.bytes(b"\xff\x00%61bc x"), b"[redacted] x".to_vec());
    }

    #[test]
    fn an_error_keeps_its_kind_and_loses_the_value() {
        let r = Redactor::new(["key-one"]);
        let e =
            pk_cli_core::CliError::Upstream("failed for key-one".into()).with_remedy("not key-one");
        let e = r.error(e);
        assert_eq!(e.exit_code(), 5);
        assert!(!e.to_string().contains("key-one"));
        assert_eq!(e.remedy(), Some("not [redacted]"));
        // A value in the remedy alone is caught too.
        let e = r.error(pk_cli_core::CliError::Auth("refused".into()).with_remedy("key-one"));
        assert_eq!(e.exit_code(), 3);
        assert_eq!(e.remedy(), Some("[redacted]"));
    }

    #[test]
    fn empty_values_redact_nothing() {
        let r = Redactor::new([""]);
        assert_eq!(r.text("abc %61"), "abc %61");
    }
}

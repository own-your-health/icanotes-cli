//! Keeping the API key and secret out of everything printed.
//!
//! ICANotes takes both as query parameters, so any text that could carry a
//! request URL (an error, a passthrough's answer) passes through a
//! [`Redactor`] first. reqwest's own errors are taken `without_url`, so this
//! is the second line, not the only one.

/// What a redacted value is replaced with.
pub const REDACTED: &str = "[redacted]";

/// Replaces each of its values, as written and percent-encoded, with
/// [`REDACTED`].
#[derive(Default, Clone)]
pub struct Redactor {
    needles: Vec<String>,
}

impl std::fmt::Debug for Redactor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Redactor({} values)", self.needles.len())
    }
}

impl Redactor {
    pub fn new<'a>(values: impl IntoIterator<Item = &'a str>) -> Self {
        let mut needles = Vec::new();
        for v in values {
            if v.is_empty() {
                continue;
            }
            let encoded: String = url::form_urlencoded::byte_serialize(v.as_bytes()).collect();
            let percent = encoded.replace('+', "%20");
            for n in [v.to_string(), encoded, percent] {
                if !needles.contains(&n) {
                    needles.push(n);
                }
            }
        }
        // Longest first, so a value that contains another is replaced whole.
        needles.sort_by_key(|n| std::cmp::Reverse(n.len()));
        Redactor { needles }
    }

    pub fn text(&self, s: &str) -> String {
        let mut out = s.to_string();
        for n in &self.needles {
            if out.contains(n.as_str()) {
                out = out.replace(n.as_str(), REDACTED);
            }
        }
        out
    }

    /// Bytes, for an answer that may not be text: replaced where the value
    /// appears as written.
    pub fn bytes(&self, b: &[u8]) -> Vec<u8> {
        let mut out = b.to_vec();
        for n in &self.needles {
            let n = n.as_bytes();
            if n.is_empty() || out.len() < n.len() {
                continue;
            }
            let mut next = Vec::with_capacity(out.len());
            let mut i = 0;
            while i < out.len() {
                if out[i..].starts_with(n) {
                    next.extend_from_slice(REDACTED.as_bytes());
                    i += n.len();
                } else {
                    next.push(out[i]);
                    i += 1;
                }
            }
            out = next;
        }
        out
    }

    pub fn error(&self, e: pk_cli_core::CliError) -> pk_cli_core::CliError {
        let message = e.message().to_string();
        let clean = self.text(&message);
        if clean == message {
            return e;
        }
        let remedy = e.remedy().map(|r| self.text(r));
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
            Some(r) => base.with_remedy(r),
            None => base,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_is_replaced_as_written_and_encoded() {
        let r = Redactor::new(["key-one", "s3cr/t+x=="]);
        let s =
            r.text("https://h/API/X?userKey=key-one&userSecret=s3cr%2Ft%2Bx%3D%3D and s3cr/t+x==");
        assert!(!s.contains("key-one"), "{s}");
        assert!(!s.contains("s3cr"), "{s}");
        assert_eq!(s.matches(REDACTED).count(), 3, "{s}");
    }

    #[test]
    fn bytes_are_redacted_too() {
        let r = Redactor::new(["abc"]);
        assert_eq!(r.bytes(b"xabcyabc"), b"x[redacted]y[redacted]".to_vec());
        assert_eq!(r.bytes(b"ab"), b"ab".to_vec());
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
    }

    #[test]
    fn empty_values_redact_nothing() {
        let r = Redactor::new([""]);
        assert_eq!(r.text("abc"), "abc");
    }
}

//! A stand-in for ICANotes' patient API on 127.0.0.1, serving the synthetic
//! fixtures, for dogfooding the installed binary without a real key:
//!
//! ```sh
//! cargo run --example mock_icanotes -- [PORT] [--limit N --reset SECS]
//! icanotes config set api_base <the printed API base>
//! printf '%s' '<the printed pair>' | icanotes auth set-credential --profile demo --stdin
//! ```
//!
//! It knows two keys (`tests/support/mock.rs` has them), both for synthetic
//! patients, and prints the first. `--fault ENDPOINT=STATUS` makes one
//! endpoint fail.

#[path = "../tests/support/mock.rs"]
mod mock;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut port = 0u16;
    let mut faults = Vec::new();
    let mut window = None;
    let mut limit = 10u32;
    let mut reset = 600i64;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--fault" => {
                let f = args.next().expect("--fault ENDPOINT=STATUS");
                let (e, s) = f.split_once('=').expect("--fault ENDPOINT=STATUS");
                faults.push((e.to_string(), s.parse::<u16>().expect("a status")));
            }
            "--limit" => {
                limit = args.next().and_then(|v| v.parse().ok()).expect("--limit N");
                window = Some(());
            }
            "--reset" => {
                reset = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .expect("--reset SECS");
                window = Some(());
            }
            p => port = p.parse().expect("a port"),
        }
    }
    let m = mock::Mock::start_on(port);
    if window.is_some() {
        m.window(limit, reset);
    }
    for (e, s) in faults {
        m.fault(&e, s);
    }
    println!("API base: {}", m.api_base());
    println!("Synthetic pair: {}:{}", mock::KEY, mock::SECRET);
    loop {
        std::thread::park();
    }
}

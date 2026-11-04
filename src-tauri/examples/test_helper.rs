//! A harmless program for the tests. It is built by `cargo test` and never shipped.
//!
//! * With a path argument it writes a marker file there.
//! * With no argument it writes `<its own path>.ran` next to itself, so a test that launches a
//!   copy of it can tell that it ran.
//! * Standard input is copied to standard output, so it also works as a no-op Git filter.

use std::io::{Read, Write};

fn main() {
    let marker = std::env::args_os().nth(1).map(std::path::PathBuf::from).or_else(|| {
        std::env::current_exe().ok().map(|exe| {
            let mut name = exe.into_os_string();
            name.push(".ran");
            name.into()
        })
    });
    if let Some(marker) = marker {
        let _ = std::fs::write(marker, b"ran");
    }
    let mut input = Vec::new();
    if std::io::stdin().read_to_end(&mut input).is_ok() {
        let _ = std::io::stdout().write_all(&input);
    }
}

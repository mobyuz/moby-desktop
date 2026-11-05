//! A harmless program for the tests. It is built by `cargo test` and never shipped.
//!
//! * With a path argument that is not a folder it writes a marker file there.
//! * With no argument it writes `<its own path>.ran` next to itself, so a test that launches a
//!   copy of it can tell that it ran.
//! * With a folder argument it does the same and records the argument in that file, so a test can
//!   check exactly what it was given.
//! * Standard input is copied to standard output, so it also works as a no-op Git filter.

use std::io::{Read, Write};
use std::path::PathBuf;

fn main() {
    let arg = std::env::args_os().nth(1).map(PathBuf::from);
    let own = std::env::current_exe().ok().map(|exe| {
        let mut name = exe.into_os_string();
        name.push(".ran");
        PathBuf::from(name)
    });
    match arg {
        Some(path) if !path.is_dir() => {
            let _ = std::fs::write(path, b"ran");
        }
        Some(folder) => {
            if let Some(own) = own {
                let _ = std::fs::write(own, folder.to_string_lossy().as_bytes());
            }
        }
        None => {
            if let Some(own) = own {
                let _ = std::fs::write(own, b"ran");
            }
        }
    }
    let mut input = Vec::new();
    if std::io::stdin().read_to_end(&mut input).is_ok() {
        let _ = std::io::stdout().write_all(&input);
    }
}

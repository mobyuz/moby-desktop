//! `open_app` and `open_project`: start a registered `.exe`. These are the only places that start
//! a program, and both run only after on-screen confirmation.
//!
//! * `std::process::Command` calls `CreateProcessW` directly. There is no shell, no `cmd /c`, no
//!   `start`, and no search of `PATH`: the program is always the absolute, canonical path.
//! * Rust turns a `.bat` or `.cmd` target into a `cmd.exe` call, so the extension, the canonical
//!   name and the `MZ` header are all checked again here, with the file held open so it cannot be
//!   swapped between the check and the launch.
//! * `open_app` passes no arguments. `open_project` passes exactly one: the canonical folder path,
//!   which always begins with a drive letter and `:\`. A leading `-` is therefore impossible, and
//!   it is checked anyway so a future change cannot break that.
//! * The working directory is the program's own folder. Standard streams are closed.

use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};

use moby_protocol::ToolFailureCode;

use crate::paths::{self, PathError, VerifiedDir, VerifiedProgram};

const DETACHED_PROCESS: u32 = 0x0000_0008;
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

fn code_for(error: PathError) -> ToolFailureCode {
    match error {
        PathError::Missing | PathError::Changed | PathError::Reparse | PathError::WrongKind => {
            ToolFailureCode::Unavailable
        }
        _ => ToolFailureCode::InvalidRequest,
    }
}

fn start(stored: &str, folder: Option<&VerifiedDir>) -> Result<(), ToolFailureCode> {
    let (_lock, program): (_, VerifiedProgram) = paths::lock_program(stored).map_err(code_for)?;
    let mut command = Command::new(program.os());
    if let Some(folder) = folder {
        let arg = folder.path.as_str();
        if arg.starts_with('-')
            || paths::check_form(arg).is_err()
            || !arg.as_bytes().get(1).is_some_and(|b| *b == b':')
        {
            return Err(ToolFailureCode::InvalidRequest);
        }
        command.arg(arg);
    }
    command
        .current_dir(program.parent())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    command
        .spawn()
        .map(drop)
        .map_err(|_| ToolFailureCode::Failed)
}

/// Starts a registered program with no arguments.
pub fn open_app(stored_exe: &str) -> Result<(), ToolFailureCode> {
    start(stored_exe, None)
}

/// Opens a folder with the chosen program, or in Windows Explorer when none is chosen.
pub fn open_project(stored_exe: Option<&str>, folder: &VerifiedDir) -> Result<(), ToolFailureCode> {
    match stored_exe {
        Some(exe) => start(exe, Some(folder)),
        None => {
            let root = std::env::var("SystemRoot").map_err(|_| ToolFailureCode::Unavailable)?;
            let explorer =
                paths::canonical_of(&format!("{}\\explorer.exe", root.trim_end_matches('\\')))
                    .map_err(code_for)?;
            start(&explorer, Some(folder))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::testing::{Scratch, helper_exe};
    use std::time::{Duration, Instant};

    fn installed(scratch: &Scratch, name: &str) -> String {
        let target = scratch.path(name);
        std::fs::copy(helper_exe(), &target).unwrap();
        target.to_str().unwrap().to_owned()
    }

    fn ran(exe: &str) -> bool {
        let marker = format!("{exe}.ran");
        let until = Instant::now() + Duration::from_secs(10);
        while Instant::now() < until {
            if std::path::Path::new(&marker).exists() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    fn not_ran(exe: &str) -> bool {
        std::thread::sleep(Duration::from_millis(400));
        !std::path::Path::new(&format!("{exe}.ran")).exists()
    }

    #[test]
    fn a_registered_program_starts_with_the_lock_held() {
        let scratch = Scratch::new();
        let exe = installed(&scratch, "tool.exe");
        let registered = paths::register_program(&exe).unwrap();
        open_app(&registered.path).unwrap();
        assert!(
            ran(&exe),
            "the harmless helper should have written its marker"
        );
    }

    #[test]
    fn scripts_shortcuts_and_swapped_files_never_start() {
        let scratch = Scratch::new();
        let marker = scratch.path("pwned.txt");
        for (name, body) in [
            (
                "a.bat",
                format!("@echo off\r\necho x> \"{}\"\r\n", marker.display()),
            ),
            (
                "a.cmd",
                format!("@echo off\r\necho x> \"{}\"\r\n", marker.display()),
            ),
            ("a.ps1", format!("Set-Content \"{}\" x", marker.display())),
        ] {
            let path = scratch.file(name, body.as_bytes());
            assert!(open_app(path.to_str().unwrap()).is_err(), "{name}");
        }
        std::thread::sleep(Duration::from_millis(500));
        assert!(!marker.exists(), "no script ran");

        // Registered as a real program, then replaced by a script with the same name.
        let exe = installed(&scratch, "swap.exe");
        let registered = paths::register_program(&exe).unwrap();
        std::fs::write(
            &exe,
            format!(
                "@echo off\r\necho x> \"{}\"\r\n{}",
                marker.display(),
                " ".repeat(100)
            ),
        )
        .unwrap();
        assert!(open_app(&registered.path).is_err());
        std::thread::sleep(Duration::from_millis(500));
        assert!(!marker.exists());
    }

    #[test]
    fn a_missing_or_hostile_stored_path_starts_nothing() {
        let scratch = Scratch::new();
        let exe = installed(&scratch, "gone.exe");
        std::fs::remove_file(&exe).unwrap();
        assert_eq!(open_app(&exe), Err(ToolFailureCode::Unavailable));
        for bad in [
            r"\\server\share\x.exe",
            r"C:\x\..\y.exe",
            "relative.exe",
            r"C:\x\nul.exe",
            r"C:\x\a.exe:stream",
            "cmd.exe",
            "",
        ] {
            assert!(open_app(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn open_project_passes_exactly_the_canonical_folder() {
        let scratch = Scratch::new();
        let exe = installed(&scratch, "ide.exe");
        let folder = scratch.dir("-dash project");
        let verified = paths::register_dir(folder.to_str().unwrap()).unwrap();
        open_project(
            Some(&paths::register_program(&exe).unwrap().path),
            &verified,
        )
        .unwrap();
        assert!(ran(&exe));
        assert_eq!(
            std::fs::read_to_string(format!("{exe}.ran")).unwrap(),
            verified.path,
            "exactly the canonical folder, nothing else"
        );
        assert!(
            verified.path.as_bytes()[1] == b':',
            "the argument starts with a drive, never a dash"
        );
        let dashed = VerifiedDir { path: "-rf".into() };
        assert_eq!(
            open_project(Some(&exe), &dashed),
            Err(ToolFailureCode::InvalidRequest)
        );
        let second = installed(&scratch, "ide2.exe");
        assert_eq!(
            open_project(Some(&second), &dashed),
            Err(ToolFailureCode::InvalidRequest)
        );
        assert!(not_ran(&second));
    }
}

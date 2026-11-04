//! Windows path hardening. Every folder or program Moby may touch is a path the person picked in
//! a native dialog, stored in its canonical form, and checked again against the disk right before
//! each use (the folder can be swapped for a junction after it was registered).
//!
//! A path is accepted only as `X:\dir\file`: a drive letter, backslashes, and components that are
//! plain names. `std::fs::canonicalize` returns `\\?\X:\...`, so that prefix is removed on purpose
//! from our own output while a `\\?\` path in anything we are *given* is refused.
//!
//! Rejected: UNC and device (`\\.\`) paths, `..`, alternate data streams (`:`), reserved device
//! names such as `CON` or `nul.txt`, trailing dots and spaces, control and bidirectional
//! characters, drive roots, and anything whose canonical form differs from the stored form
//! (a junction, a symlink, a renamed parent, or an 8.3 short name that points elsewhere).

use std::fs;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use crate::text::is_unsafe_char;

/// Below `MAX_PATH`, so the plain form works for programs that cannot take `\\?\` paths.
pub const MAX_PATH_CHARS: usize = 240;
const REPARSE_POINT: u32 = 0x400;
const FILE_SHARE_READ: u32 = 0x1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathError {
    /// Not `X:\...`: relative, UNC, device, verbatim, rooted, forward slashes, or not text.
    Form,
    /// A component Windows treats specially, or a character that cannot be in a plain name.
    Name,
    TooLong,
    /// A drive root. Registering a whole drive is never what a person means.
    Root,
    Missing,
    WrongKind,
    /// The path now leads somewhere other than where it did when it was stored.
    Changed,
    /// A link or junction.
    Reparse,
    NotProgram,
    Io,
}

/// A directory checked against the disk just now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedDir {
    /// `X:\dir`, as stored.
    pub path: String,
}

impl VerifiedDir {
    pub fn os(&self) -> PathBuf {
        PathBuf::from(&self.path)
    }

    /// The `\\?\` form, which also works beyond `MAX_PATH`. Safe because the path is already
    /// fully normalised: no `.`, no `..`, no forward slashes.
    pub fn verbatim(&self) -> PathBuf {
        PathBuf::from(format!(r"\\?\{}", self.path))
    }
}

/// A program checked against the disk just now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedProgram {
    pub path: String,
}

impl VerifiedProgram {
    pub fn os(&self) -> PathBuf {
        PathBuf::from(&self.path)
    }

    pub fn parent(&self) -> PathBuf {
        Path::new(&self.path)
            .parent()
            .map_or_else(PathBuf::new, Path::to_path_buf)
    }
}

fn is_device_name(component: &str) -> bool {
    // Windows ignores the extension and trailing spaces: "NUL.txt" and "con .x" are the device.
    let stem = component.split('.').next().unwrap_or("").trim_end_matches(' ');
    let upper = stem.to_uppercase();
    if matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" | "CLOCK$") {
        return true;
    }
    let mut chars = upper.chars();
    let prefix: String = chars.by_ref().take(3).collect();
    matches!(prefix.as_str(), "COM" | "LPT")
        && matches!(chars.next(), Some('0'..='9' | '\u{B9}' | '\u{B2}' | '\u{B3}'))
        && chars.next().is_none()
}

fn check_component(component: &str) -> Result<(), PathError> {
    if component.is_empty()
        || component == "."
        || component == ".."
        || component.ends_with('.')
        || component.ends_with(' ')
        || component.chars().any(|c| matches!(c, '<' | '>' | ':' | '"' | '/' | '|' | '?' | '*') || is_unsafe_char(c))
        || is_device_name(component)
    {
        return Err(PathError::Name);
    }
    Ok(())
}

/// Checks the form of a path without touching the disk. `X:\a\b` only.
pub fn check_form(path: &str) -> Result<(), PathError> {
    let mut parts = path.split('\\');
    let drive = parts.next().unwrap_or("");
    let mut letters = drive.chars();
    let valid_drive = matches!((letters.next(), letters.next(), letters.next()), (Some(l), Some(':'), None) if l.is_ascii_alphabetic());
    if !valid_drive {
        return Err(PathError::Form);
    }
    if path.len() == 3 && path.ends_with('\\') {
        return Err(PathError::Root);
    }
    let mut count = 0;
    for component in parts {
        check_component(component).map_err(|_| {
            // Doubled or trailing backslashes leave an empty component.
            if component.is_empty() { PathError::Form } else { PathError::Name }
        })?;
        count += 1;
    }
    if count == 0 {
        return Err(PathError::Root);
    }
    if path.chars().count() > MAX_PATH_CHARS {
        return Err(PathError::TooLong);
    }
    Ok(())
}

/// Removes the `\\?\` that `canonicalize` adds, but only for a plain drive path. `\\?\UNC\...`,
/// volume GUID paths and anything else stay refused.
fn simplify(canonical: &Path) -> Result<String, PathError> {
    let text = canonical.to_str().ok_or(PathError::Form)?;
    let plain = text.strip_prefix(r"\\?\").ok_or(PathError::Form)?;
    check_form(plain)?;
    Ok(plain.to_owned())
}

fn same_path(a: &str, b: &str) -> bool {
    a == b || a.to_lowercase() == b.to_lowercase()
}

pub fn is_reparse(meta: &fs::Metadata) -> bool {
    meta.file_attributes() & REPARSE_POINT != 0 || meta.file_type().is_symlink()
}

/// The canonical form of something that exists: resolves links, `..`, short names and case.
/// The input must already have the form `X:\...`.
pub fn canonical_of(input: &str) -> Result<String, PathError> {
    check_form(input)?;
    let canonical = fs::canonicalize(input).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => PathError::Missing,
        _ => PathError::Io,
    })?;
    simplify(&canonical)
}

/// Canonical form of a path a person just picked. Unlike [`verify_dir`], a link in the way is
/// followed once, because the person sees and chooses the result that gets stored.
pub fn register_dir(input: &str) -> Result<VerifiedDir, PathError> {
    let canonical = canonical_of(input)?;
    verify_dir(&canonical)
}

/// A program a person just picked. Only `.exe` files are accepted.
pub fn register_program(input: &str) -> Result<VerifiedProgram, PathError> {
    let canonical = canonical_of(input)?;
    verify_program(&canonical)
}

/// Checks a stored directory against the disk: it exists, is a real directory, and its canonical
/// form is still exactly the stored one. Run before every use.
pub fn verify_dir(stored: &str) -> Result<VerifiedDir, PathError> {
    let canonical = canonical_of(stored)?;
    if !same_path(&canonical, stored) {
        return Err(PathError::Changed);
    }
    let meta = fs::symlink_metadata(&canonical).map_err(|_| PathError::Missing)?;
    if is_reparse(&meta) {
        return Err(PathError::Reparse);
    }
    if !meta.is_dir() {
        return Err(PathError::WrongKind);
    }
    Ok(VerifiedDir { path: canonical })
}

fn has_exe_extension(path: &str) -> bool {
    let name = path.rsplit('\\').next().unwrap_or("");
    name.len() > 4 && name.get(name.len() - 4..).is_some_and(|ext| ext.eq_ignore_ascii_case(".exe"))
}

/// Checks a stored program against the disk: a real file named `*.exe`, not a link, starting with
/// the `MZ` header of a Windows executable. Batch files, scripts, shortcuts and everything else
/// are refused here, and this runs again at launch.
pub fn verify_program(stored: &str) -> Result<VerifiedProgram, PathError> {
    if !has_exe_extension(stored) {
        return Err(PathError::NotProgram);
    }
    let canonical = canonical_of(stored)?;
    if !same_path(&canonical, stored) {
        return Err(PathError::Changed);
    }
    // The canonical name decides, not the one we were given: a link named x.exe may lead to x.bat.
    if !has_exe_extension(&canonical) {
        return Err(PathError::NotProgram);
    }
    let meta = fs::symlink_metadata(&canonical).map_err(|_| PathError::Missing)?;
    if is_reparse(&meta) {
        return Err(PathError::Reparse);
    }
    if !meta.is_file() || meta.len() < 64 {
        return Err(PathError::WrongKind);
    }
    let mut header = [0_u8; 2];
    let mut file = fs::File::open(&canonical).map_err(|_| PathError::Io)?;
    std::io::Read::read_exact(&mut file, &mut header).map_err(|_| PathError::NotProgram)?;
    if &header != b"MZ" {
        return Err(PathError::NotProgram);
    }
    Ok(VerifiedProgram { path: canonical })
}

/// Holds a program open so it cannot be replaced, renamed or deleted (and neither can the
/// folders above it) between the last check and the launch.
pub struct ProgramLock {
    _file: fs::File,
}

/// Locks the program, then verifies it. The order matters: after the lock is held the check
/// cannot go stale.
pub fn lock_program(stored: &str) -> Result<(ProgramLock, VerifiedProgram), PathError> {
    check_form(stored)?;
    let file = fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(stored)
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => PathError::Missing,
            _ => PathError::Io,
        })?;
    let verified = verify_program(stored)?;
    Ok((ProgramLock { _file: file }, verified))
}

/// A short, local-only description of why a path was refused. Never includes the path.
pub fn explain(error: PathError) -> &'static str {
    match error {
        PathError::Form => "that isn't a normal local path",
        PathError::Name => "a name in that path isn't allowed",
        PathError::TooLong => "that path is too long",
        PathError::Root => "a whole drive can't be added",
        PathError::Missing => "it no longer exists",
        PathError::WrongKind => "it isn't the right kind of item",
        PathError::Changed => "it now points somewhere else",
        PathError::Reparse => "it is a link or junction",
        PathError::NotProgram => "only .exe programs can be added",
        PathError::Io => "Windows would not open it",
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use uuid::Uuid;

    /// A scratch folder under the system temp dir (set to D:\moby-cache\tmp by the caller).
    pub struct Scratch(pub PathBuf);

    impl Scratch {
        pub fn new() -> Self {
            let base = std::env::temp_dir().join(format!("moby-paths-{}", Uuid::new_v4()));
            std::fs::create_dir_all(&base).unwrap();
            // Tests compare canonical forms, so start from one.
            Self(std::fs::canonicalize(&base).unwrap().to_str().unwrap().trim_start_matches(r"\\?\").into())
        }

        pub fn path(&self, rel: &str) -> PathBuf {
            self.0.join(rel.replace('/', "\\"))
        }

        pub fn dir(&self, rel: &str) -> PathBuf {
            let path = self.path(rel);
            std::fs::create_dir_all(&path).unwrap();
            path
        }

        pub fn file(&self, rel: &str, bytes: &[u8]) -> PathBuf {
            let path = self.path(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&path, bytes).unwrap();
            path
        }

        pub fn s(&self, rel: &str) -> String {
            self.path(rel).to_str().unwrap().to_owned()
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            // A junction must be removed as a junction: remove_dir_all would try to follow some.
            let _ = Command::new("cmd").args(["/C", "rd", "/S", "/Q"]).arg(&self.0).output();
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// `mklink /J link target`. Needs no privilege.
    pub fn junction(link: &Path, target: &Path) {
        let out = Command::new("cmd").args(["/C", "mklink", "/J"]).arg(link).arg(target).output().unwrap();
        assert!(out.status.success(), "mklink failed: {}", String::from_utf8_lossy(&out.stdout));
    }

    /// Removes a junction without touching its target.
    pub fn remove_junction(link: &Path) {
        std::fs::remove_dir(link).unwrap();
    }

    /// The helper program built from `examples/test_helper.rs`. Panics with a hint if it is missing.
    pub fn helper_exe() -> PathBuf {
        let exe = std::env::current_exe().unwrap();
        let target = exe.parent().unwrap().parent().unwrap();
        let helper = target.join("examples").join("test_helper.exe");
        assert!(helper.exists(), "build it first: cargo test builds examples; missing {}", helper.display());
        std::fs::canonicalize(helper).unwrap().to_str().unwrap().trim_start_matches(r"\\?\").into()
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    #[test]
    fn plain_drive_paths_pass_the_form_check() {
        for ok in [r"C:\Users\me\proj", r"d:\a b\c.d", r"C:\a\..b", "C:\\caf\u{e9}\\\u{4e2d}\u{6587}", r"C:\PROGRA~1\x"] {
            assert_eq!(check_form(ok), Ok(()), "{ok}");
        }
    }

    #[test]
    fn the_hostile_path_corpus_is_refused_by_form() {
        let corpus: &[(&str, PathError)] = &[
            (r"\\server\share\x", PathError::Form),
            (r"\\?\C:\x", PathError::Form),
            (r"\\.\C:\x", PathError::Form),
            (r"\\.\PhysicalDrive0", PathError::Form),
            (r"\\?\UNC\server\share\x", PathError::Form),
            (r"\\?\Volume{01234567-0123-0123-0123-0123456789ab}\x", PathError::Form),
            ("C:/x/y", PathError::Form),
            (r"C:x", PathError::Form),
            (r"\x", PathError::Form),
            (r"x\y", PathError::Form),
            ("", PathError::Form),
            ("C:", PathError::Root),
            (r"C:\", PathError::Root),
            (r"C:\x\", PathError::Form),
            (r"C:\x\..\y", PathError::Name),
            (r"C:\x\.\y", PathError::Name),
            (r"C:\x:stream", PathError::Name),
            (r"C:\x\file.txt:$DATA", PathError::Name),
            (r"C:\x\file.txt::$DATA", PathError::Name),
            (r"C:\x\CON", PathError::Name),
            (r"C:\x\con.txt", PathError::Name),
            (r"C:\x\NUL", PathError::Name),
            (r"C:\x\nul.tar.gz", PathError::Name),
            (r"C:\x\AUX", PathError::Name),
            (r"C:\x\PRN.log", PathError::Name),
            (r"C:\x\COM1", PathError::Name),
            (r"C:\x\com9.txt", PathError::Name),
            (r"C:\x\LPT1", PathError::Name),
            (r"C:\x\lpt3.dat", PathError::Name),
            ("C:\\x\\COM\u{b9}", PathError::Name),
            (r"C:\x\CONIN$", PathError::Name),
            (r"C:\x\CONOUT$", PathError::Name),
            (r"C:\x\nul .txt", PathError::Name),
            (r"C:\x.", PathError::Name),
            (r"C:\x \y", PathError::Name),
            (r"C:\x\y.", PathError::Name),
            (r"C:\x\y ", PathError::Name),
            (r"C:\x\a|b", PathError::Name),
            (r"C:\x\a*b", PathError::Name),
            (r"C:\x\a?b", PathError::Name),
            (r"C:\x\a<b", PathError::Name),
            ("C:\\x\\a\"b", PathError::Name),
            ("C:\\x\\a\u{0}b", PathError::Name),
            ("C:\\x\\a\nb", PathError::Name),
            ("C:\\x\\a\u{202E}b", PathError::Name),
            ("C:\\x\\a\u{200B}b", PathError::Name),
            (r"C:\x\\y", PathError::Form),
            ("1:\\x", PathError::Form),
        ];
        for (path, expected) in corpus {
            assert_eq!(check_form(path), Err(*expected), "{path:?}");
        }
        assert_eq!(check_form(&format!("C:\\{}", "a".repeat(260))), Err(PathError::TooLong));
    }

    #[test]
    fn device_names_are_matched_the_way_windows_does() {
        for name in ["CON", "con", "Nul.txt", "COM1", "lpt9", "COM0", "aux.x.y", "CONIN$"] {
            assert!(is_device_name(name), "{name}");
        }
        for name in ["CONSOLE", "COM10", "COMX", "LPT", "console.txt", "nu", "com", "a.con"] {
            assert!(!is_device_name(name), "{name}");
        }
    }

    #[test]
    fn a_real_directory_registers_in_plain_canonical_form() {
        let scratch = Scratch::new();
        let dir = scratch.dir("proj");
        let registered = register_dir(dir.to_str().unwrap()).unwrap();
        assert!(!registered.path.starts_with(r"\\"), "our own output has no \\\\?\\ prefix");
        assert!(same_path(&registered.path, dir.to_str().unwrap()));
        assert!(registered.verbatim().to_str().unwrap().starts_with(r"\\?\"));
        assert_eq!(verify_dir(&registered.path), Ok(registered.clone()));
        assert_eq!(register_dir(scratch.path("missing").to_str().unwrap()), Err(PathError::Missing));
    }

    #[test]
    fn case_changes_are_the_same_place() {
        let scratch = Scratch::new();
        let dir = scratch.dir("MixedCase");
        let registered = register_dir(dir.to_str().unwrap()).unwrap();
        let shouted = registered.path.to_uppercase();
        let verified = verify_dir(&shouted).unwrap();
        assert!(same_path(&verified.path, &registered.path));
    }

    #[test]
    fn a_file_is_not_a_directory_and_a_directory_is_not_a_program() {
        let scratch = Scratch::new();
        let file = scratch.file("plain.txt", b"hello");
        let dir = scratch.dir("folder.exe");
        assert_eq!(register_dir(file.to_str().unwrap()), Err(PathError::WrongKind));
        assert_eq!(register_program(dir.to_str().unwrap()), Err(PathError::WrongKind));
    }

    #[test]
    fn a_junction_out_of_the_root_is_not_followed_silently() {
        let scratch = Scratch::new();
        let root = scratch.dir("root");
        let outside = scratch.dir("outside");
        scratch.file("outside/secret.txt", b"secret");
        junction(&root.join("link"), &outside);

        // Registered as picked, a junction resolves to its target, which is what the person sees.
        let through = register_dir(root.join("link").to_str().unwrap()).unwrap();
        assert!(same_path(&through.path, outside.to_str().unwrap()));
        // But the stored junction path itself no longer verifies: it leads somewhere else.
        let stored = root.join("link").to_str().unwrap().to_owned();
        assert_eq!(verify_dir(&stored), Err(PathError::Changed));
        remove_junction(&root.join("link"));
    }

    #[test]
    fn a_directory_swapped_for_a_junction_after_registration_is_caught() {
        let scratch = Scratch::new();
        let root = scratch.dir("project");
        let elsewhere = scratch.dir("elsewhere");
        let registered = register_dir(root.to_str().unwrap()).unwrap();
        assert!(verify_dir(&registered.path).is_ok());

        std::fs::remove_dir(&root).unwrap();
        junction(&root, &elsewhere);
        assert!(matches!(verify_dir(&registered.path), Err(PathError::Changed | PathError::Reparse)));
        remove_junction(&root);

        // A parent swapped out from under a deeper registration is caught the same way.
        let deep = scratch.dir("a/b/c");
        let stored = register_dir(deep.to_str().unwrap()).unwrap();
        std::fs::remove_dir_all(scratch.path("a")).unwrap();
        junction(&scratch.path("a"), &elsewhere);
        scratch.dir("elsewhere/b/c");
        assert!(verify_dir(&stored.path).is_err());
        remove_junction(&scratch.path("a"));
    }

    #[test]
    fn a_renamed_or_deleted_directory_no_longer_verifies() {
        let scratch = Scratch::new();
        let root = scratch.dir("gone");
        let registered = register_dir(root.to_str().unwrap()).unwrap();
        std::fs::remove_dir(&root).unwrap();
        assert_eq!(verify_dir(&registered.path), Err(PathError::Missing));
    }

    #[test]
    fn short_names_that_change_identity_are_caught() {
        let scratch = Scratch::new();
        let long = scratch.dir("a long directory name");
        let out = std::process::Command::new("cmd")
            .args(["/C", &format!("for %I in (\"{}\") do @echo %~sI", long.display())])
            .output()
            .unwrap();
        let short = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        if !short.contains('~') || same_path(&short, long.to_str().unwrap()) {
            eprintln!("8.3 names are off on this volume; skipping the short-name check");
            return;
        }
                assert_eq!(verify_dir(&short), Err(PathError::Changed));
        let registered = register_dir(&short).unwrap();
        assert!(same_path(&registered.path, long.to_str().unwrap()), "registration stores the long form");
    }

    #[test]
    fn alternate_data_streams_never_reach_the_disk() {
        let scratch = Scratch::new();
        let file = scratch.file("note.txt", b"x");
        let stream = format!("{}:hidden", file.display());
        std::fs::write(&stream, b"secret stream").unwrap();
        assert_eq!(register_dir(&stream), Err(PathError::Name));
        assert_eq!(verify_program(&format!("{}:evil.exe", file.display())), Err(PathError::Name));
        assert_eq!(verify_dir(&stream), Err(PathError::Name));
    }

    fn fake_pe() -> Vec<u8> {
        let mut bytes = b"MZ".to_vec();
        bytes.resize(256, 0);
        bytes
    }

    #[test]
    fn only_real_exe_files_are_programs() {
        let scratch = Scratch::new();
        let good = scratch.file("tool.exe", &fake_pe());
        assert!(register_program(good.to_str().unwrap()).is_ok());
        assert!(register_program(&good.to_str().unwrap().to_uppercase()).is_ok());

        for name in ["a.bat", "a.cmd", "a.ps1", "a.lnk", "a.com", "a.vbs", "a.js", "a.msi", "a.scr", "a.exe.txt", "a.txt", "a"] {
            let path = scratch.file(name, &fake_pe());
            assert_eq!(register_program(path.to_str().unwrap()), Err(PathError::NotProgram), "{name}");
        }
        let imposter = scratch.file("notreal.exe", b"echo this is a script, not a PE file ......................................");
        assert_eq!(register_program(imposter.to_str().unwrap()), Err(PathError::NotProgram));
        let tiny = scratch.file("tiny.exe", b"MZ");
        assert_eq!(register_program(tiny.to_str().unwrap()), Err(PathError::WrongKind));
    }

    #[test]
    fn a_program_swapped_after_registration_is_caught_at_launch_time() {
        let scratch = Scratch::new();
        let dir = scratch.dir("apps");
        let exe = scratch.file("apps/tool.exe", &fake_pe());
        let registered = register_program(exe.to_str().unwrap()).unwrap();

        // Replaced by a script under the same name: no longer a PE file.
        std::fs::write(&exe, b"@echo off\r\necho pwned ................................................................").unwrap();
        assert_eq!(verify_program(&registered.path), Err(PathError::NotProgram));

        // The whole folder swapped for a junction to another folder holding a valid program.
        std::fs::write(&exe, fake_pe()).unwrap();
        assert!(verify_program(&registered.path).is_ok());
        let other = scratch.dir("other");
        scratch.file("other/tool.exe", &fake_pe());
        std::fs::remove_dir_all(&dir).unwrap();
        junction(&dir, &other);
        assert!(matches!(verify_program(&registered.path), Err(PathError::Changed | PathError::Reparse)));
        remove_junction(&dir);
    }

    #[test]
    fn a_locked_program_cannot_be_replaced_until_the_lock_is_dropped() {
        let scratch = Scratch::new();
        let exe = scratch.file("locked.exe", &fake_pe());
        let registered = register_program(exe.to_str().unwrap()).unwrap();
        let (lock, verified) = lock_program(&registered.path).unwrap();
        assert_eq!(verified, registered);
        assert!(std::fs::write(&exe, b"replacement").is_err(), "writes are refused while locked");
        assert!(std::fs::remove_file(&exe).is_err(), "deleting is refused while locked");
        assert!(std::fs::rename(&exe, scratch.path("moved.exe")).is_err(), "renaming is refused while locked");
        drop(lock);
        assert!(std::fs::remove_file(&exe).is_ok());
    }
}

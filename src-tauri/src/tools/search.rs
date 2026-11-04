//! `search_files`: file names only, a literal substring (no patterns, no contents), inside one
//! registered folder. The walk never follows a link or junction, and it stops at fixed limits.

use std::collections::VecDeque;
use std::os::windows::fs::MetadataExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use moby_protocol::ToolFailureCode;

use crate::paths::{self, VerifiedDir};
use crate::text;

pub const MAX_ENTRIES: usize = 100_000;
pub const MAX_DEPTH: usize = 16;

/// Relative paths (with backslashes) of entries whose name contains `query`, case-insensitively.
/// Fewer than `max_results` found because a limit ended the walk is `PartialResult`.
pub fn search(root: &VerifiedDir, query: &str, max_results: usize, cancel: &AtomicBool, deadline: Instant) -> Result<Vec<String>, ToolFailureCode> {
    search_with_limits(root, query, max_results, cancel, deadline, MAX_ENTRIES, MAX_DEPTH)
}

pub(crate) fn search_with_limits(
    root: &VerifiedDir,
    query: &str,
    max_results: usize,
    cancel: &AtomicBool,
    deadline: Instant,
    max_entries: usize,
    max_depth: usize,
) -> Result<Vec<String>, ToolFailureCode> {
    let needle = query.to_lowercase();
    let mut found = Vec::new();
    let mut queue = VecDeque::from([(root.verbatim(), String::new(), 0_usize)]);
    let mut seen = 0_usize;
    let mut truncated = false;
    while let Some((dir, rel, depth)) = queue.pop_front() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            truncated = true;
            continue;
        };
        for entry in entries {
            if cancel.load(Ordering::Relaxed) {
                return Err(ToolFailureCode::Canceled);
            }
            if Instant::now() >= deadline {
                return Err(ToolFailureCode::Timeout);
            }
            let Ok(entry) = entry else {
                truncated = true;
                continue;
            };
            seen += 1;
            if seen > max_entries {
                return if found.len() >= max_results { Ok(found) } else { Err(ToolFailureCode::PartialResult) };
            }
            // Metadata of the entry itself, never of what a link points to.
            let Ok(meta) = entry.metadata() else {
                truncated = true;
                continue;
            };
            if paths::is_reparse(&meta) {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let child_rel = if rel.is_empty() { name.clone() } else { format!("{rel}\\{name}") };
            if name.to_lowercase().contains(&needle) {
                found.push(text::short_line(&child_rel, 120));
                if found.len() >= max_results {
                    return Ok(found);
                }
            }
            if meta.is_dir() && meta.file_attributes() & 0x400 == 0 {
                if depth + 1 > max_depth {
                    truncated = true;
                } else {
                    queue.push_back((entry.path(), child_rel, depth + 1));
                }
            }
        }
    }
    if truncated { Err(ToolFailureCode::PartialResult) } else { Ok(found) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::testing::{Scratch, junction, remove_junction};
    use std::time::Duration;

    fn run(root: &std::path::Path, query: &str, max: usize) -> Result<Vec<String>, ToolFailureCode> {
        let verified = paths::register_dir(root.to_str().unwrap()).unwrap();
        search(&verified, query, max, &AtomicBool::new(false), Instant::now() + Duration::from_secs(10))
    }

    #[test]
    fn finds_names_by_literal_substring_ignoring_case() {
        let scratch = Scratch::new();
        let root = scratch.dir("root");
        scratch.file("root/src/Main.rs", b"fn main() {}");
        scratch.file("root/readme.md", b"needle in contents only");
        scratch.file("root/a+b (1).txt", b"x");
        let found = run(&root, "main", 10).unwrap();
        assert_eq!(found, ["src\\Main.rs"]);
        assert!(run(&root, "needle", 10).unwrap().is_empty(), "contents are never searched");
        assert_eq!(run(&root, "a+b (1)", 10).unwrap(), ["a+b (1).txt"], "no pattern syntax");
        assert!(run(&root, ".*", 10).unwrap().is_empty(), "a regex is just text");
        assert!(run(&root, "*", 10).unwrap().is_empty(), "a glob is just text");
    }

    #[test]
    fn stops_at_the_result_limit() {
        let scratch = Scratch::new();
        let root = scratch.dir("root");
        for i in 0..10 {
            scratch.file(&format!("root/hit{i}.txt"), b"x");
        }
        assert_eq!(run(&root, "hit", 3).unwrap().len(), 3);
    }

    #[test]
    fn never_follows_a_junction_out_of_the_root() {
        let scratch = Scratch::new();
        let root = scratch.dir("root");
        let outside = scratch.dir("outside");
        scratch.file("outside/secret-token.txt", b"x");
        scratch.file("root/inside.txt", b"x");
        junction(&root.join("exit"), &outside);
        assert!(run(&root, "secret", 10).unwrap().is_empty());
        assert!(run(&root, "exit", 10).unwrap().is_empty(), "the link itself is skipped too");
        assert_eq!(run(&root, "inside", 10).unwrap().len(), 1);
        remove_junction(&root.join("exit"));
    }

    #[test]
    fn limits_make_the_answer_partial_and_deadlines_and_cancel_are_honoured() {
        let scratch = Scratch::new();
        let root = scratch.dir("root");
        for i in 0..20 {
            scratch.file(&format!("root/f{i}.txt"), b"x");
        }
        scratch.file("root/a/b/c/deep-hit.txt", b"x");
        let verified = paths::register_dir(root.to_str().unwrap()).unwrap();
        let cancel = AtomicBool::new(false);
        let later = Instant::now() + Duration::from_secs(10);
        assert_eq!(search_with_limits(&verified, "zzz", 5, &cancel, later, 5, 10), Err(ToolFailureCode::PartialResult));
        assert_eq!(search_with_limits(&verified, "zzz", 5, &cancel, later, 1000, 1), Err(ToolFailureCode::PartialResult));
        assert_eq!(search_with_limits(&verified, "deep-hit", 5, &cancel, later, 1000, 10).unwrap().len(), 1);
        assert_eq!(search_with_limits(&verified, "f", 5, &cancel, Instant::now(), 1000, 10), Err(ToolFailureCode::Timeout));
        cancel.store(true, Ordering::Relaxed);
        assert_eq!(search_with_limits(&verified, "f", 5, &cancel, later, 1000, 10), Err(ToolFailureCode::Canceled));
    }

    #[test]
    fn hostile_names_come_back_clean() {
        let scratch = Scratch::new();
        let root = scratch.dir("root");
        scratch.file("root/evil\u{202E}name\u{200B}.txt", b"x");
        let found = run(&root, "evil", 10).unwrap();
        assert_eq!(found, ["evilname.txt"]);
    }
}

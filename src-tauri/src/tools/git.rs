//! `git_status`, read only, in this process with gitoxide (`gix`). No `git.exe` is started.
//!
//! The repository belongs to somebody else as far as this code is concerned: its config, its
//! `.gitattributes`, its hooks and its file names are all untrusted. See
//! `docs/adr/0004-git-status.md` for the evidence behind each defence below.
//!
//! * `Options::isolated()` reads nothing but the repository's own config and no environment, and
//!   never runs `git` to find its installation config.
//! * `filter_config_section` drops every `[filter "x"]` section that comes from the repository, so
//!   `filter.x.clean`/`smudge`/`process` can never become a program to run. `.gitattributes` can
//!   name a driver, but a driver that is not configured is ignored.
//! * Submodules are never opened (their config would be one more untrusted source), so a
//!   repository with submodules answers `PartialResult` instead of a possibly wrong status.
//! * `.git` must be a plain directory inside the registered folder and `core.worktree` must not
//!   point elsewhere.
//! * gix has no code path in status that reads `core.fsmonitor`, hooks or `core.sshCommand`.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use moby_protocol::ToolFailureCode;

use crate::paths::{self, VerifiedDir};
use crate::text;

/// Names listed in the answer; the rest is only counted.
pub const MAX_NAMES: usize = 20;
/// Past this many changes the walk stops and the answer is partial.
const MAX_ITEMS: usize = 20_000;
/// Commits counted when comparing with the upstream branch.
const MAX_WALK: usize = 5_000;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub branch: String,
    pub detached: bool,
    pub upstream: Option<String>,
    pub ahead: Option<usize>,
    pub behind: Option<usize>,
    pub staged: usize,
    pub unstaged: usize,
    pub untracked: usize,
    pub conflicted: usize,
    /// `(status letter, relative path)`, at most [`MAX_NAMES`], already cleaned.
    pub names: Vec<(char, String)>,
    pub state: Option<&'static str>,
}

impl Summary {
    pub fn total(&self) -> usize {
        self.staged + self.unstaged + self.untracked + self.conflicted
    }

    /// The text for the model and for the person. File and branch names come from the repository,
    /// so the caller fences the whole of it as untrusted.
    pub fn render(&self) -> String {
        let mut out = String::new();
        if self.detached {
            out.push_str(&format!("branch: (detached HEAD at {})\n", self.branch));
        } else {
            out.push_str(&format!("branch: {}\n", self.branch));
        }
        match (&self.upstream, self.ahead, self.behind) {
            (Some(upstream), Some(ahead), Some(behind)) => {
                out.push_str(&format!(
                    "upstream: {upstream} (ahead {ahead}, behind {behind})\n"
                ));
            }
            (Some(upstream), _, _) => {
                out.push_str(&format!("upstream: {upstream} (not compared)\n"))
            }
            _ => out.push_str("upstream: none\n"),
        }
        if let Some(state) = self.state {
            out.push_str(&format!("in progress: {state}\n"));
        }
        out.push_str(&format!(
            "changed files: {} (staged {}, not staged {}, untracked {}, conflicts {})\n",
            self.total(),
            self.staged,
            self.unstaged,
            self.untracked,
            self.conflicted
        ));
        for (letter, name) in &self.names {
            out.push_str(&format!("{letter} {name}\n"));
        }
        if self.total() > self.names.len() {
            out.push_str(&format!(
                "... and {} more\n",
                self.total() - self.names.len()
            ));
        }
        out
    }
}

/// Drops every config section that came from the repository itself. Used as gix's
/// `filter_config_section`, which decides what may define filter drivers.
fn not_from_the_repository(meta: &gix::config::file::Metadata) -> bool {
    meta.source.kind() != gix::config::source::Kind::Repository
}

/// How the repository is opened. Public inside the crate so a test can show what happens with
/// gix's default options instead.
pub(crate) fn safe_open_options() -> gix::open::Options {
    gix::open::Options::isolated().filter_config_section(not_from_the_repository)
}

fn is_plain_dir(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir() && !paths::is_reparse(&meta))
}

fn clean_name(raw: &[u8], limit: usize) -> String {
    text::short_line(&String::from_utf8_lossy(raw), limit)
}

fn letter_for_worktree(
    status: &gix::status::plumbing::index_as_worktree::EntryStatus<(), gix::submodule::Status>,
) -> Option<(char, Kind)> {
    use gix::status::plumbing::index_as_worktree::{Change, EntryStatus};
    match status {
        EntryStatus::Conflict { .. } => Some(('U', Kind::Conflicted)),
        EntryStatus::IntentToAdd => Some(('A', Kind::Unstaged)),
        EntryStatus::NeedsUpdate(_) => None,
        EntryStatus::Change(Change::Removed) => Some(('D', Kind::Unstaged)),
        EntryStatus::Change(_) => Some(('M', Kind::Unstaged)),
    }
}

enum Kind {
    Staged,
    Unstaged,
    Untracked,
    Conflicted,
}

/// Status of the registered project. `cancel` is shared with the caller, who sets it on timeout
/// or when the person cancels; the walk checks it between steps.
pub fn status(
    project: &VerifiedDir,
    cancel: &Arc<AtomicBool>,
    deadline: Instant,
) -> Result<Summary, ToolFailureCode> {
    status_with(project, cancel, deadline, MAX_WALK)
}

pub(crate) fn status_with(
    project: &VerifiedDir,
    cancel: &Arc<AtomicBool>,
    deadline: Instant,
    max_walk: usize,
) -> Result<Summary, ToolFailureCode> {
    check_stop(cancel, deadline)?;
    // `.git` must be a real folder right inside the project: not a link, not a `gitdir:` file that
    // points somewhere we did not register.
    let dot_git = Path::new(&project.path).join(".git");
    if !is_plain_dir(&dot_git) {
        return Err(ToolFailureCode::Unavailable);
    }
    // Files inside `.git` that make gix read another folder (possibly a network share).
    refuse_redirects(&dot_git)?;
    check_stop(cancel, deadline)?;
    let repo =
        gix::open_opts(project.os(), safe_open_options()).map_err(|_| ToolFailureCode::Failed)?;
    if repo.is_bare() || !same_place(repo.common_dir(), repo.git_dir()) {
        return Err(ToolFailureCode::Unavailable);
    }
    check_stop(cancel, deadline)?;
    // `core.worktree` can move the working tree anywhere. The answer must be about the registered folder.
    let workdir = repo
        .workdir()
        .and_then(|dir| dir.to_str())
        .and_then(|dir| paths::canonical_of(dir.trim_end_matches(['\\', '/'])).ok());
    let git_dir = repo
        .git_dir()
        .to_str()
        .and_then(|dir| paths::canonical_of(dir.trim_end_matches(['\\', '/'])).ok());
    let expected_git_dir = paths::canonical_of(dot_git.to_str().unwrap_or("")).ok();
    if !workdir
        .as_deref()
        .is_some_and(|dir| dir.eq_ignore_ascii_case(&project.path))
        || git_dir.is_none()
        || git_dir != expected_git_dir
    {
        return Err(ToolFailureCode::Unavailable);
    }

    let mut summary = Summary {
        state: repo
            .state()
            .map(|_| "an operation such as a merge or rebase"),
        ..Summary::default()
    };
    summary.branch = match repo.head_name() {
        Ok(Some(name)) => clean_name(name.shorten(), 80),
        Ok(None) => {
            summary.detached = true;
            repo.head_id().ok().map_or_else(
                || "unknown".to_owned(),
                |id| id.to_hex_with_len(8).to_string(),
            )
        }
        Err(_) => return Err(ToolFailureCode::Failed),
    };

    // A submodule is a repository we deliberately do not open, so its state is unknown.
    // Reading the index cannot be interrupted once started; the checks around it bound the damage.
    let index = repo.index_or_empty().map_err(|_| ToolFailureCode::Failed)?;
    check_stop(cancel, deadline)?;
    let has_submodules = index
        .entries()
        .iter()
        .any(|entry| entry.mode == gix::index::entry::Mode::COMMIT);
    drop(index);
    if has_submodules {
        return Err(ToolFailureCode::PartialResult);
    }

    let iter = repo
        .status(gix::progress::Discard)
        .map_err(|_| ToolFailureCode::Failed)?
        .index_worktree_submodules(gix::status::Submodule::Given {
            ignore: gix::submodule::config::Ignore::All,
            check_dirty: false,
        })
        .untracked_files(gix::status::UntrackedFiles::Collapsed)
        .should_interrupt_owned(Arc::clone(cancel))
        .into_iter(std::iter::empty::<gix::bstr::BString>())
        .map_err(|_| ToolFailureCode::Failed)?;

    let mut seen = 0_usize;
    for item in iter {
        check_stop(cancel, deadline)?;
        let item = item.map_err(|_| ToolFailureCode::PartialResult)?;
        seen += 1;
        if seen > MAX_ITEMS {
            return Err(ToolFailureCode::PartialResult);
        }
        let (letter, kind) = match &item {
            gix::status::Item::TreeIndex(change) => {
                let letter = match change {
                    gix::diff::index::Change::Addition { .. } => 'A',
                    gix::diff::index::Change::Deletion { .. } => 'D',
                    gix::diff::index::Change::Modification { .. } => 'M',
                    gix::diff::index::Change::Rewrite { .. } => 'R',
                };
                (letter, Kind::Staged)
            }
            gix::status::Item::IndexWorktree(gix::status::index_worktree::Item::Modification {
                status,
                ..
            }) => match letter_for_worktree(status) {
                Some(found) => found,
                None => continue,
            },
            gix::status::Item::IndexWorktree(
                gix::status::index_worktree::Item::DirectoryContents { entry, .. },
            ) => {
                if entry.status == gix::dir::entry::Status::Ignored(gix::ignore::Kind::Expendable)
                    || matches!(entry.status, gix::dir::entry::Status::Ignored(_))
                {
                    continue;
                }
                ('?', Kind::Untracked)
            }
            gix::status::Item::IndexWorktree(gix::status::index_worktree::Item::Rewrite {
                ..
            }) => ('R', Kind::Unstaged),
        };
        match kind {
            Kind::Staged => summary.staged += 1,
            Kind::Unstaged => summary.unstaged += 1,
            Kind::Untracked => summary.untracked += 1,
            Kind::Conflicted => summary.conflicted += 1,
        }
        if summary.names.len() < MAX_NAMES {
            summary
                .names
                .push((letter, clean_name(item.location().as_ref(), 120)));
        }
    }
    check_stop(cancel, deadline)?;

    compare_with_upstream(&repo, &mut summary, cancel, deadline, max_walk)?;
    Ok(summary)
}

fn check_stop(cancel: &AtomicBool, deadline: Instant) -> Result<(), ToolFailureCode> {
    if cancel.load(Ordering::Relaxed) {
        Err(ToolFailureCode::Canceled)
    } else if Instant::now() >= deadline {
        Err(ToolFailureCode::Timeout)
    } else {
        Ok(())
    }
}

fn same_place(a: &Path, b: &Path) -> bool {
    let canon = |p: &Path| {
        p.to_str()
            .and_then(|s| paths::canonical_of(s.trim_end_matches(['\\', '/'])).ok())
    };
    matches!((canon(a), canon(b)), (Some(x), Some(y)) if x.eq_ignore_ascii_case(&y))
}

/// Entries of the repository folder that gix follows to wherever their contents point, or that
/// could be links to another place. Checked without following links. Any of them means this is not
/// a plain, self-contained repository, and nothing is opened.
fn refuse_redirects(dot_git: &Path) -> Result<(), ToolFailureCode> {
    // Files that name another folder: a shared repository, a linked worktree, borrowed objects.
    for name in ["commondir", "gitdir", "objects\\info\\alternates"] {
        match std::fs::symlink_metadata(dot_git.join(name)) {
            Ok(_) => return Err(ToolFailureCode::Unavailable),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(ToolFailureCode::Unavailable),
        }
    }
    // Links among the folders gix reads from. The top level of `.git` and of `objects`, and the
    // whole of `refs` up to a bound.
    let mut budget = 5000_usize;
    let mut stack = vec![
        (dot_git.to_path_buf(), 1_u8),
        (dot_git.join("objects"), 1),
        (dot_git.join("refs"), 6),
    ];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            budget = budget
                .checked_sub(1)
                .ok_or(ToolFailureCode::PartialResult)?;
            let Ok(meta) = entry.metadata() else {
                return Err(ToolFailureCode::Unavailable);
            };
            if paths::is_reparse(&meta) {
                return Err(ToolFailureCode::Unavailable);
            }
            if meta.is_dir() && depth > 1 {
                stack.push((entry.path(), depth - 1));
            }
        }
    }
    Ok(())
}

/// Fills in the upstream name and the ahead/behind counts when the branch tracks something. A
/// failure here leaves the counts out instead of failing the whole answer.
fn compare_with_upstream(
    repo: &gix::Repository,
    summary: &mut Summary,
    cancel: &AtomicBool,
    deadline: Instant,
    max_walk: usize,
) -> Result<(), ToolFailureCode> {
    if summary.detached {
        return Ok(());
    }
    let Ok(Some(head)) = repo.head_name() else {
        return Ok(());
    };
    // gix's own lookup ignores repository config because of the filter above, so the two plain
    // settings are read here. They only name a reference; nothing is run from them.
    let short = head.shorten().to_string();
    let config = repo.config_snapshot();
    let (Some(remote), Some(merge)) = (
        config.string(format!("branch.{short}.remote").as_str()),
        config.string(format!("branch.{short}.merge").as_str()),
    ) else {
        return Ok(());
    };
    let (remote, merge) = (remote.to_string(), merge.to_string());
    let Some(branch) = merge.strip_prefix("refs/heads/") else {
        return Ok(());
    };
    if remote == "." || remote.is_empty() || remote.contains(['\\', ':', ' ']) {
        return Ok(());
    }
    let Ok(upstream_name) =
        gix::refs::FullName::try_from(format!("refs/remotes/{remote}/{branch}"))
    else {
        return Ok(());
    };
    summary.upstream = Some(clean_name(upstream_name.shorten(), 80));
    let (Ok(local), Ok(mut upstream)) =
        (repo.head_id(), repo.find_reference(upstream_name.as_ref()))
    else {
        return Ok(());
    };
    let Ok(upstream) = upstream.peel_to_id() else {
        return Ok(());
    };
    // The walk with a hidden tip may read many commits before it yields the first one, and that
    // part cannot be interrupted. Between commits it stops on cancel and deadline, and past
    // `max_walk` commits the whole answer is partial.
    let count =
        |from: gix::ObjectId, hidden: gix::ObjectId| -> Result<Option<usize>, ToolFailureCode> {
            let Ok(walk) = repo.rev_walk([from]).with_hidden([hidden]).all() else {
                return Ok(None);
            };
            let mut n = 0;
            for commit in walk {
                check_stop(cancel, deadline)?;
                if commit.is_err() {
                    return Ok(None);
                }
                n += 1;
                if n > max_walk {
                    return Err(ToolFailureCode::PartialResult);
                }
            }
            Ok(Some(n))
        };
    summary.ahead = count(local.detach(), upstream.detach())?;
    summary.behind = count(upstream.detach(), local.detach())?;
    Ok(())
}

#[cfg(test)]
#[path = "git_tests.rs"]
mod tests;

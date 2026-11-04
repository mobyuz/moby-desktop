# 0004 In-process git status (D5)

Status: accepted. Date: 2026-11-04.

## Context

`git_status` has to answer "what changed in this project" for a folder the person registered. The repository is not trusted: it may be a fresh clone of somebody else's code, and its `.git/config`, `.gitattributes`, hooks and file names are all attacker-controlled text. The owner decided to try an in-process library before shelling out to `git.exe`.

## Decision

Use `gix` (gitoxide) **0.90.0, pinned exactly**, with only the features `status`, `revision` and `sha1`. No network, credentials, commit-signing, blame, merge or worktree-mutation features are compiled in. Nothing starts `git.exe`.

## Can gix status start another program? (evidence)

Read from the 0.90.0 sources and the crates they pull in (`gix-filter 0.36.1`, `gix-diff 0.70.0`, `gix-path 0.14.1`, `gix-status 0.37.0`, `gix-command 0.12.1`). Every place in the compiled graph that spawns a process, outside tests:

| Spawn site | Reached by status? | What we do |
| --- | --- | --- |
| `gix-filter` `driver/init.rs` (`filter.<name>.clean/smudge/process`) | **Yes.** Status compares content through the "to git" filter pipeline, and `Repository::filter` builds the drivers from config in `src/filter.rs::extract_drivers`. With default options a repository the user owns is fully trusted, so its `[filter "x"] clean = ...` is run. The test `the_same_repository_does_start_the_filter_with_gix_default_options` shows this happening. | `open::Options::filter_config_section` rejects every config section whose source is the repository (`Kind::Repository`). `extract_drivers` then finds no driver, and `gix-filter` ignores an attribute that names an unknown driver (`util.rs::extract_driver`). |
| `gix-diff` `blob/pipeline.rs` and `platform.rs` (`diff.<name>.textconv`, `diff.external`) | No. The status pipeline runs in `Mode::ToGit`; textconv and external diff run only when a caller asks for them. | Not used. |
| `gix-path` `env/git/mod.rs`, `env/mod.rs` (runs `git` to find the installation config; finds a shell) | Only when system or git-binary config is loaded. | `Options::isolated()`: `git_binary`, `system`, `git`, `user`, `env` and `includes` are all off. |
| `gix` `commit/sign.rs`, `repository/config/mod.rs::editor_command` | No: signing and editors. | Not used. |
| `gix-status` `fscache.rs` (`fsutil.exe`, `cmd.exe`) | No: they are in its test module. | n/a |

`core.fsmonitor`: gix reads only the FSMN *extension* of the index (a list of flags). There is no code that runs a fsmonitor hook or `watchman`. Hooks (`.git/hooks`, `core.hooksPath`) are never run by anything in the read path. `core.sshCommand` is for transports, which are not compiled in. `diff.external`, `core.editor`, `core.pager` have no reader here.

## Other defences

- `.git` must be a real directory directly inside the registered folder, not a link and not a `gitdir:` file, so a repository cannot point status at some other place. Worktrees and submodule checkouts therefore answer `Unavailable`.
- The opened repository's working tree and git directory are compared with the registered folder after canonicalisation. (gix 0.90 does not follow `core.worktree` from this repository; the test accepts either `Unavailable` or an answer that contains nothing from the other folder.)
- Submodules are never opened, so a project with a gitlink entry answers `PartialResult` instead of a possibly wrong answer.
- Branch, upstream and file names go through the same text cleaning as everything else (control, bidirectional and invisible characters out, length limits) and the whole answer is fenced as untrusted data, at most 8 KiB.
- Upstream: gix's own tracking lookup uses the same filtered config, so it finds nothing. The two settings it needs, `branch.<name>.remote` and `.merge`, are read directly; they only name a reference.
- Work stops at a deadline and on cancel through gix's interrupt flag, and the walk gives up after 20,000 changes with `PartialResult`.

## Proof

`src-tauri/src/tools/git_tests.rs` builds a repository that tries to start the harmless test helper through `filter.<name>.clean` and `smudge` (with `.gitattributes: * filter=evil`), `core.fsmonitor`, `core.sshCommand`, `core.editor`, `core.pager`, `core.hooksPath` and repository hooks, `diff.<name>.textconv` and `command`, `diff.external`, an `include.path` file, and an ssh remote. The tracked file is changed to the same size so it has to be compared by content. The helper writes a marker file when it runs.

- With `safe_open_options()` the status is correct and the marker never appears.
- With gix's default options, over the same repository, the marker **does** appear. That control keeps the first test from passing for the wrong reason.

The test does not use the `filter.<name>.process` key: the long-running filter protocol makes the helper wait for input forever, and the setting is dropped by the same mechanism as `clean`.

## Limits

- Not done: submodule status, linked worktrees, rename detection, ignored files. These answer `PartialResult` or `Unavailable`, or are omitted by design.
- `gix` is a large dependency tree (about 90 crates). It is pinned, and an upgrade is a reviewed change that must re-run the proof above.
- Untracked directories are shown collapsed (`dir/`), as `git status` does, and are counted as one.

## Alternatives

| Option | Why not |
| --- | --- |
| `git.exe status --porcelain` | Runs git with the repository's config: `core.fsmonitor`, filters and `core.sshCommand` are the classic ways a hostile repository runs code. It would need `-c` overrides for each, an allowlist that has to keep up with git, and a git installation. |
| `libgit2` (`git2`) | A C dependency; its status also applies filter drivers in some builds, and it adds a build requirement. |
| Reading `.git/index` by hand | Wrong answers (racy timestamps, line-ending conversion, ignore rules) are worse than no answer. |

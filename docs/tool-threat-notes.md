# Local tools: threat notes and Definition of Done

The model's output is an untrusted request, never permission. Everything below assumes the model, the API stream, repository contents, file and folder names, and the webview are all possible sources of hostile input.

## Who can do what

| Actor | Can | Cannot |
| --- | --- | --- |
| The person | Add and remove folders and programs, grant and revoke reads, answer dialogs, delete reminders | n/a |
| The webview | Send conversation text, stop a turn, approve or deny a dialog Rust issued, list its own registry | Name a path or id for a tool, start a tool, create a dialog, answer one from another account, read the audit of another account |
| The model | Ask for an advertised tool with an advertised id | Choose a path, a program, arguments or a shell; skip a check; approve itself |
| A repository or folder | Contain names, text, config, hooks | Cause a program to run (git_status ignores its filters, hooks and commands; nothing opens a file's contents) |

## Per tool

| Tool | Worst plausible abuse | Defence | Test |
| --- | --- | --- | --- |
| `git_status` | A hostile repo config or attributes run a program; a `core.worktree` or gitfile exposes other folders; names carry instructions | ADR 0004: isolated gix, repository filter sections dropped, `.git` must be a plain folder, submodules refused, output cleaned and fenced | `tools/git_tests.rs` (trap repository with control), agent injection test |
| `search_files` | Escape the root through a junction or `..`; read contents; flood the model | Literal substring on names only; the walk never follows a reparse point; limits on entries (100,000), depth (16), 10 s, results (50); query already refused if it has separators or control characters | `tools/search.rs` tests, bad-call corpus |
| `open_app` | Launch something other than the registered program; use `.bat`, `.lnk`, a swapped file or a `PATH` lookup | Only `.exe` with an `MZ` header, absolute canonical path, no arguments, no shell, working directory is the program's folder, the file is held open (no write or delete sharing) from the check to `CreateProcess`, checked again at launch | `paths.rs` and `tools/launch.rs` tests; agent tests (denied, expired, canceled never launch) |
| `open_project` | An option injected as the folder argument; launching the wrong program | Exactly one argument, the canonical folder path which always begins with a drive letter and `:\`, so it can never start with `-`, and it is verified to be so; same launch rules as `open_app` | `tools/launch.rs::open_project_passes_exactly_the_canonical_folder` (uses a folder named `-dash project`) |
| `create_reminder` | A reminder whose text misleads, or hides characters | On-screen confirmation of the cleaned text in quotes; control, bidirectional and invisible characters removed; due time must be in the future and within a year; at most 50 pending | agent reminder tests |

## Cross-cutting

- **Paths** (`paths.rs`): only `X:\...`; no UNC, `\\?\`, `\\.\`, `..`, alternate data streams, device names (with extensions and trailing spaces), trailing dot or space, control or bidirectional characters, drive roots; canonical form must equal the stored form, case-insensitively (a junction, symlink, rename or short-name swap changes it). `std::fs::canonicalize` returns `\\?\C:\...`; the prefix is removed from our own output only, and a `\\?\` path in input is refused.
- **TOCTOU**: every use calls `verify_dir` or `verify_program` again after any confirmation. The approval is also compared with a fresh resolution of the same request, so a registry change while the dialog is open ends in `Unavailable`.
- **Approvals**: ids are random, single use, tied to account and sign-in, expire in two minutes into a denial, and die with a cancel or sign-out. The buttons that allow are disabled for 600 ms after the dialog appears; the dialog starts on Deny.
- **Audit**: metadata only. Fixed tokens and ids, never names, paths, queries, messages or output. Retention and delete-all apply to it; the activity screen can clear it.
- **Output to the model**: control, bidirectional, invisible, tag and variation-selector characters removed; the fence markers cannot occur inside; at most 8 KiB in total; labelled as untrusted data.
- **Resources sent to the API**: ids and display names only. Display names are cleaned and at most 100 characters. No paths.

## Known gaps

- A process running as the same Windows user can edit the database, the registry rows and the program files directly. These controls defend against hostile content, not against that.
- A registered IDE can run project tasks of its own (for example a workspace trust prompt); that is the IDE's decision after launch.
- Session grants for launches skip the dialog for up to 8 hours once chosen.
- Notification text is shown by Windows outside the app's window.
- `git_status` does not support submodules, linked worktrees or rename detection.
- Only one turn runs at a time; a second is refused with `busy`.

## Definition of Done for a new tool

1. The tool exists in `moby-protocol` (`ToolName`, `ToolRequest`) with `deny_unknown_fields` and bounded fields, and the protocol docs list it.
2. A row in the table above: what the worst abuse is, the defence, the test.
3. Resolution goes through the registry by id for the signed-in account; no path, program or argument comes from the request.
4. `tools::plan` decides read grant, session grant or confirmation, and the approval sentence comes from `Action::describe`. A tool that changes anything outside Moby never runs on a permanent grant.
5. `tools::execute_blocking` takes a cancel flag and a deadline, verifies paths again, uses no shell, fixes its arguments, and returns a `ToolFailureCode` for every failure. Output goes through `text::untrusted_output`.
6. An audit entry for every outcome, with new decision words added to `DECISION_LABELS` and tokens only.
7. `build_offer` in `agent.rs` advertises it only when something is registered and granted.
8. Tests: a path or input escape corpus, a bad-call case, an injection case where names carry instructions, a denied and a canceled case that prove nothing ran, a grant expiry and revocation case, a timeout and a cancel case.
9. `capabilities/default.json` still has no permissions.
10. README, CHANGELOG and this file are updated, and the change gets an adversarial review.

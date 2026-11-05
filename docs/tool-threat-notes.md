# Local tools: threat notes and Definition of Done

The model's output is an untrusted request, never permission. Everything below assumes the model, the API stream, repository contents, file and folder names, and the webview are all possible sources of hostile input.

## Who can do what

| Actor | Can | Cannot |
| --- | --- | --- |
| The person | Add and remove folders and programs, grant and revoke reads, answer dialogs, delete reminders | n/a |
| The webview | Send conversation text, stop a turn, allow or deny a **reminder** dialog Rust issued, list its own registry (names and the last two path parts only) | Name a path or id for a tool, start a tool, approve a launch (there is nothing to answer: launches are confirmed in a native window), clear the activity log on its own, answer a dialog of another account, read another account's audit |
| The model | Ask for an advertised tool with an advertised id | Choose a path, a program, arguments or a shell; skip a check; approve itself |
| A repository or folder | Contain names, text, config, hooks | Cause a program to run (git_status ignores its filters, hooks and commands; nothing opens a file's contents) |

## Per tool

| Tool | Worst plausible abuse | Defence | Test |
| --- | --- | --- | --- |
| `git_status` | A hostile repo config or attributes run a program; `commondir`, `alternates` or `gitdir` make gix open another folder or a network share (starting authentication to an attacker's server); names carry instructions | ADR 0004: isolated gix, repository filter sections dropped, `.git` must be a plain folder with none of `commondir`, `gitdir`, `objects/info/alternates` and no links inside, `common_dir` must equal `git_dir`, submodules refused, deadline and cancel checked throughout, output cleaned and fenced | `tools/git_tests.rs` (trap repository and a positive control for each redirect), agent injection test |
| `search_files` | Escape the root through a junction or `..`; read contents; flood the model | Literal substring on names only; the walk never follows a reparse point; limits on entries (100,000), depth (16), 10 s, results (50); query already refused if it has separators or control characters | `tools/search.rs` tests, bad-call corpus |
| `open_app` | Launch something other than the registered program; use `.bat`, `.lnk`, a swapped file or a `PATH` lookup; a compromised page approves its own launch | Only `.exe` with an `MZ` header, not a shell, script host or launcher by name, absolute canonical path, no arguments, no shell, working directory is the program's folder, the file is held open (no write or delete sharing) from the check to `CreateProcess`, checked again at launch. **Every launch is confirmed in a native window owned by Rust** (OK / Cancel, Cancel is the default, real path shown, no model text), with no session or permanent grant | `paths.rs` and `tools/launch.rs` tests; agent tests (denied, expired, stopped, signed out, changed registry: never launch; the page cannot approve) |
| `open_project` | An option injected as the folder argument; launching the wrong program | Exactly one argument, the canonical folder path which always begins with a drive letter and `:\`, so it can never start with `-`, and it is verified to be so; same launch rules and native confirmation as `open_app`. The name list of shells and interpreters is defence in depth, **not a boundary**: a renamed copy passes | `tools/launch.rs::open_project_passes_exactly_the_canonical_folder` (uses a folder named `-dash project`) |
| `create_reminder` | A reminder whose text misleads, or hides characters | On-screen confirmation of the cleaned text in quotes; control, bidirectional and invisible characters removed; due time must be in the future and within a year; at most 50 pending | agent reminder tests |

## Robot link and voice (D3, D4)

| Threat | Defence | Test |
| --- | --- | --- |
| A device on the port sends `pair_reset` to erase the pairing | The key is kept; the session is untrusted until the person unpairs or pairs again | `a_forged_pair_reset_cannot_erase_a_pairing_or_start_audio` |
| A non-Moby USB device is opened and written to | Native confirmation with name and USB IDs before anything is written | `a_port_that_is_not_a_moby_needs_a_native_yes_that_shows_its_ids` |
| A hello flood burns Credential Manager reads or keeps a dead link alive | 500 ms minimum gap; a hello is not a heartbeat | `a_hello_flood_costs_one_vault_read_and_hellos_alone_cannot_keep_a_link_alive` |
| Robot text hides characters (bidi, zero-width) | `text.rs` on the robot's error text and device labels | `text_from_the_robot_loses_bidi_and_zero_width_characters` |
| A compromised page turns cloud voice on | Native confirmation of the consent in Rust | `turning_voice_on_needs_the_native_yes_and_cancel_records_nothing` |
| A stream trickles in forever | 30 s wall clock on one stream | `a_stream_that_trickles_in_is_cut_at_the_wall_clock_limit` |
| A canceled or revoked turn still sends a request | Explicit cancel check and biased `select!`; revoke stops the turn first | `a_turn_that_is_already_canceled_never_sends_its_request`, `revoking_stops_the_turn_first_...` |
| A spoken "yes" approves a tool | Voice has no path to the approval code; launches need the native window | `a_spoken_yes_never_approves_a_reminder`, `a_spoken_yes_never_launches_a_program_the_native_window_decides` |

| A device copies the robot ID, says it is paired, answers keepalives with `command_ok` and sends audio | The robot must send a valid `session_proof` (HMAC over the pairing key, both nonces, device and session) within 3 s of the ack; only that authenticates; no audio, no upload, no agent turn without it | `a_forged_device_with_the_real_id_that_answers_keepalives_and_sends_audio_reaches_nothing` |
| The robot proof is reflected, replayed, computed with the wrong nonce, late or missing | Fresh `desktop_nonce` per ack, constant-time check against this session, first proof only, 3 s window, label that keeps the two proofs apart | `robots_that_do_not_prove_the_key_are_unproven_and_get_no_audio`, `a_proof_from_an_old_session_is_no_use_on_a_new_one`, the protocol vector tests |
| `hello` with `paired: false` for an ID whose key is saved | Stale, not new: key kept, nothing offered, no audio | `a_hello_that_says_unpaired_for_an_id_whose_key_is_saved_is_stale_not_new` |

Known limit: after the handshake frames carry no MAC. A device that takes over or relays the cable after a proven handshake is not stopped (protocol ADR 0005).

## Cross-cutting

- **Paths** (`paths.rs`): only `X:\...`; no UNC, `\\?\`, `\\.\`, `..`, alternate data streams, device names (with extensions and trailing spaces), trailing dot or space, control or bidirectional characters, drive roots; canonical form must equal the stored form, case-insensitively (a junction, symlink, rename or short-name swap changes it). `std::fs::canonicalize` returns `\\?\C:\...`; the prefix is removed from our own output only, and a `\\?\` path in input is refused.
- **TOCTOU**: every use calls `verify_dir` or `verify_program` again after any confirmation. The approval is also compared with a fresh resolution of the same request, so a registry change while the dialog is open ends in `Unavailable`.
- **Approvals**: launches and clearing the activity log are confirmed in a native window (`confirm.rs`, `native.rs`) that the page cannot click; OK / Cancel with Cancel as the default, closed on timeout or cancel. Reminders use the in-page dialog: ids are random, single use, tied to account and sign-in, expire in two minutes into a denial, and die with a cancel or sign-out; its Allow button is disabled for 600 ms and the dialog starts on Deny. A Stop, a sign-out or a registry change while a window is open cancels the call even if OK is pressed afterwards; the epoch changes before the turn is ended.
- **Audit**: metadata only. Fixed tokens and ids, never names, paths, queries, messages or output. Retention and delete-all apply to it; the activity screen can clear it.
- **Output to the model**: control, bidirectional, invisible, tag and variation-selector characters removed; the fence markers cannot occur inside; at most 8 KiB in total; labelled as untrusted data.
- **Resources sent to the API**: ids and display names only. Display names are cleaned and at most 100 characters. No paths. The page itself sees only the last two path components.

## Known gaps

- A process running as the same Windows user can edit the database, the registry rows and the program files directly. These controls defend against hostile content, not against that.
- A registered IDE can run project tasks of its own (for example a workspace trust prompt); that is the IDE's decision after launch.
- The native confirmation window has not been looked at by a person in review; unit tests use a stand-in. It is the only `unsafe` code in the app.
- Notification text is shown by Windows outside the app's window.
- `git_status` does not support submodules, linked worktrees or rename detection.
- Only one turn runs at a time; a second is refused with `busy`.

## Definition of Done for a new tool

1. The tool exists in `moby-protocol` (`ToolName`, `ToolRequest`) with `deny_unknown_fields` and bounded fields, and the protocol docs list it.
2. A row in the table above: what the worst abuse is, the defence, the test.
3. Resolution goes through the registry by id for the signed-in account; no path, program or argument comes from the request.
4. `tools::plan` decides read grant, session grant or confirmation, and the approval sentence comes from `Action::describe`. A tool that changes anything outside Moby is confirmed on every call, in a native window if it starts a program or deletes evidence, and never runs on a permanent or session grant (owner decision, 2026-11-05).
5. `tools::execute_blocking` takes a cancel flag and a deadline, verifies paths again, uses no shell, fixes its arguments, and returns a `ToolFailureCode` for every failure. Output goes through `text::untrusted_output`.
6. An audit entry for every outcome, with new decision words added to `DECISION_LABELS` and tokens only.
7. `build_offer` in `agent.rs` advertises it only when something is registered and granted.
8. Tests: a path or input escape corpus, a bad-call case, an injection case where names carry instructions, a denied and a canceled case that prove nothing ran, a grant expiry and revocation case, a timeout and a cancel case.
9. `capabilities/default.json` still has no permissions, and nothing the page can call returns a real path.
9a. Every new protection has a test that fails when the protection is removed; say in the change which one you removed to check.
10. README, CHANGELOG and this file are updated, and the change gets an adversarial review.

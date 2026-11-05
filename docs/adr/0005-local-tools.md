# 0005 Safe local tools, permissions, approvals and audit (D5)

Status: accepted for D5, with decisions for the owner at the end. Date: 2026-11-04.

## Decisions

- **The turn runs in Rust.** `src-tauri/src/agent.rs` builds the request, reads the stream, runs the tool loop and sends only display events to the webview. The webview sends the conversation text and the language, parsed strictly (unknown fields, a leading assistant message, tool messages are all refused). It cannot send `available_tools`, `resources`, a tool call or a result, and it has no command that names a path or starts a tool.
- **Approving a reminder is the only thing the webview can do.** `answer_approval(approval_id, decision)` resolves a pending reminder dialog that Rust issued from an API stream. It has no way to approve a launch (see the update below). The id must exist, belong to the signed-in account and the current sign-in (`Api::epoch`), and not have been answered, expired or canceled. The first answer wins; an unknown, repeated, late, cross-account or cross-sign-in answer is refused. The only decisions are Deny and Allow once.
- **Execution chain, in `tools/mod.rs` and `agent.rs`, for every call:** strict `ToolRequest` (the stream frame is parsed as `AgentTurnResponse`; anything that does not parse, repeats a `call_id`, or names a tool or id the turn did not offer ends the turn with nothing run); resolve ids through the account's registry; grant check; confirmation; run with a fixed argument list, no shell, 10 s and an 8 KiB output cap, after the paths are checked again; sanitise and fence the output; write the audit entry. Limits per turn: 4 tool rounds, 2 minutes to answer a dialog.
- **Approval text is made by the app** from the action and registry rows. The model's words from the same request appear separately as "Moby says (not checked)".
- **Failure codes.** Denied, expired, canceled, timed-out and partial outcomes are sent to the model as the protocol's `ToolFailureCode` and shown to the person in the chat.
- **Registry.** Folders and programs are picked in a native dialog opened by Rust (`rfd`, called from Rust only; no dialog, filesystem or shell plugin, empty capability set). Ids are `proj_<uuid>`, `app_<uuid>` and `root_<uuid>`; a project is its own search root (`root_X` is `proj_X`). Rows live in the existing `projects` and `allowed_apps` tables. At most 20 projects and 20 programs, so the resource list stays under the API's limit of 64 (a project plus its root count as two).
- **Grants.** Permanent grants are `permanent_read` rows, only for `git_status` and `search_files`, with optional expiry and revocation, created automatically when a project is added. Session grants exist for the two read tools only, are in memory, per account and sign-in, and end on sign-out, account deletion, app exit, or after 8 hours. Nothing that acts has a session grant. A read tool without a valid grant is refused (`Denied`) without a dialog.
- **Side-effect tools** (`open_app`, `open_project`) are confirmed on every call in a native window (owner decision of 2026-11-05, below). `create_reminder` asks each time in the app.
- **Advertising.** `available_tools` and `resources` list only what is registered and granted when the turn starts, and stay fixed for the turn because the API validates the history against them. A grant revoked mid-turn is checked again when the call runs.
- **Audit.** `audit_log` rows hold tool, resource id, decision, time, duration and result code. Every field is a fixed token (letters, digits, underscore); the store refuses anything else, so content cannot be written by mistake. The list is per account; clearing deletes that account's rows and rebuilds the file.
- **Reminders** are stored per account, checked every 15 s by a Rust timer, and shown as a Windows notification through `tauri-plugin-notification`, called from Rust only. Reminders that came due while Moby was closed or nobody was signed in become "missed" when the account signs in and are shown that way. The plugin injects its own page script; the capability set is empty, so its commands are not callable from the page.
- **Windows paths** are checked in `paths.rs` (see the threat notes) at registration and again before every use. Programs are held open with a no-write, no-delete share mode from the last check until the process is created.
- **Migration 0002** rebuilds `reminders` with the state `missed` and a notification time, and adds per-account indexes.

## D2 changes made here

- Deleting the account removes that account's local data (history, folders, programs, grants, reminders, activity) after the server confirms, and ends the session even if the local cleanup fails.
- The database moved to `app_local_data_dir` (`%LOCALAPPDATA%\uz.moby.desktop`). An existing file in the roaming folder is moved once at startup. ADR 0003 and the README are updated.
- Delete-all history says in the interface that it covers every account of this Windows user.
- The activity log's clear button works.

## Owner decision, 2026-11-05: every launch is confirmed on its own

The owner decided that **every `open_app` and `open_project` launch needs its own confirmation**. There is no "always allow", no "this session", and no 8-hour launch grant. The code has no path that launches without a fresh answer: `tools::plan` returns `Confirm` for every launch and nothing in it consults a grant. `open_project` without an app still opens Windows Explorer.

## Security review fixes, 2026-11-05

- **Launch confirmation moved out of the webview.** A launch is confirmed in a native Windows message box that Rust opens (`src/native.rs`, `MessageBoxW`), with text Rust writes: the app's sentence, the real folder and program paths, no model text. The buttons are OK and Cancel with **Cancel as the default** (`MB_OKCANCEL | MB_DEFBUTTON2`); Escape and the close button also answer Cancel. `rfd`'s message dialog was not used because it gives no control over the default button and Windows defaults to the first one. The webview is not shown a dialog for a launch, receives no approval id, and `answer_approval` finds nothing to approve; it is only told (`native_prompt`) that a window is open, so the face can look attentive. The window is behind a `Confirmer` trait so tests need no real window. A timeout or cancel closes the window. Text is cleaned of control, bidirectional and invisible characters and cut to 1,200 characters. **The native window was not exercised by a person in this review**; only the trait is tested.
- **`clear_activity`** is confirmed the same way, so a compromised page cannot wipe the evidence. The in-page confirmation stays as a courtesy.
- **Stop, sign-out and the order of events.** `handle_call` treats a cancel as "not current", checks it after the answer, again after the re-check of the registry, and once more before running. Sign-out changes the sign-in epoch first and then ends the turn (`begin_sign_out`), and so does account deletion. A launch that was cut off by a timeout or cancel after the program had started is reported and audited as started.
- **Program names.** Shells, script hosts, interpreters and launchers (`cmd`, `powershell`, `pwsh`, `wscript`, `cscript`, `mshta`, `rundll32`, `regsvr32`, `bash`, `wsl`, `python`, `node`, `conhost`, `wt` and others, see `paths.rs`) are refused by canonical file name when registering and again at launch. This is defence in depth for `open_project`, whose one argument is a folder path; it is **not** a boundary, because a renamed copy passes.
- **Paths no longer reach the page.** `list_registry`, `add_project` and `add_app` return the display name and the last two path components only.
- **Read access is stated where it is granted.** The folder picker title and the Tools screen say that adding a folder lets Moby read file names and Git status, not file contents, and that it can be revoked. The behaviour (automatic permanent read grants) is unchanged.
- **Git** (ADR 0004): repositories that name another folder (`commondir`, `gitdir`, `objects/info/alternates`) or contain links inside `.git` are refused before opening.

## Other decisions for the owner

1. **Default for `open_project` without an app** is Windows Explorer (`%SystemRoot%\explorer.exe`, checked like any program). The protocol says only "default handling".
2. **The protocol is a sibling checkout** of the branch `claude/usb-control-full` (Rust path dependency), as ADR 0001 describes. CI checks out `main` of moby-protocol, which must have the tool types before CI can build this.
3. **Reminder time zone.** Reminders are stored as an absolute instant; the dialog shows the offset the model gave.
4. **Unsafe code.** `src/native.rs` is the only place with `unsafe` (three Win32 calls). Replacing it with a safe wrapper crate would need one that lets the caller choose the default button.

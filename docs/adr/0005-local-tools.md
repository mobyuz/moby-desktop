# 0005 Safe local tools, permissions, approvals and audit (D5)

Status: accepted for D5, with decisions for the owner at the end. Date: 2026-11-04.

## Decisions

- **The turn runs in Rust.** `src-tauri/src/agent.rs` builds the request, reads the stream, runs the tool loop and sends only display events to the webview. The webview sends the conversation text and the language, parsed strictly (unknown fields, a leading assistant message, tool messages are all refused). It cannot send `available_tools`, `resources`, a tool call or a result, and it has no command that names a path or starts a tool.
- **Approving is the only thing the webview can do.** `answer_approval(approval_id, decision)` resolves a pending dialog that Rust issued from an API stream. The id must exist, belong to the signed-in account and the current sign-in (`Api::epoch`), and not have been answered, expired or canceled. The first answer wins; an unknown, repeated, late, cross-account or cross-sign-in answer is refused. A decision the dialog did not offer ("this session" for a reminder) is refused.
- **Execution chain, in `tools/mod.rs` and `agent.rs`, for every call:** strict `ToolRequest` (the stream frame is parsed as `AgentTurnResponse`; anything that does not parse, repeats a `call_id`, or names a tool or id the turn did not offer ends the turn with nothing run); resolve ids through the account's registry; grant check; confirmation; run with a fixed argument list, no shell, 10 s and an 8 KiB output cap, after the paths are checked again; sanitise and fence the output; write the audit entry. Limits per turn: 4 tool rounds, 2 minutes to answer a dialog.
- **Approval text is made by the app** from the action and registry rows. The model's words from the same request appear separately as "Moby says (not checked)".
- **Failure codes.** Denied, expired, canceled, timed-out and partial outcomes are sent to the model as the protocol's `ToolFailureCode` and shown to the person in the chat.
- **Registry.** Folders and programs are picked in a native dialog opened by Rust (`rfd`, called from Rust only; no dialog, filesystem or shell plugin, empty capability set). Ids are `proj_<uuid>`, `app_<uuid>` and `root_<uuid>`; a project is its own search root (`root_X` is `proj_X`). Rows live in the existing `projects` and `allowed_apps` tables. At most 20 projects and 20 programs, so the resource list stays under the API's limit of 64 (a project plus its root count as two).
- **Grants.** Permanent grants are `permanent_read` rows, only for `git_status` and `search_files`, with optional expiry and revocation, created automatically when a project is added. Session grants are in memory, per account and sign-in, and end on sign-out, account deletion, app exit, or after 8 hours. A read tool without a valid grant is refused (`Denied`) without a dialog.
- **Side-effect tools** (`open_app`, `open_project`) ask each time or for this session. `create_reminder` asks each time. See the owner's decision below about "always allow".
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

## Decisions for the owner

1. **"Always allow" for opening apps.** The plan's tool table lets the person choose it, but its grant levels make permanent grants read-only and the hard rule asks for on-screen confirmation of launches. This stage offers *once* and *this session* only for side-effect tools. Changing that is a one-line policy decision plus a grant level.
2. **Session grants for launches** do skip the dialog for the rest of the session (up to 8 hours) once chosen. If every launch must be confirmed on screen without exception, remove "Allow for this session" for `open_app` and `open_project`.
3. **Default for `open_project` without an app** is Windows Explorer (`%SystemRoot%\explorer.exe`, checked like any program). The protocol says only "default handling".
4. **The protocol is a sibling checkout** of the branch `claude/usb-control-full` (Rust path dependency), as ADR 0001 describes. CI checks out `main` of moby-protocol, which must have the tool types before CI can build this.
5. **Reminder time zone.** Reminders are stored as an absolute instant; the dialog shows the offset the model gave.

# 0003 Local store (D2)

Status: accepted. Date: 2026-11-04.

## Context

Conversations lived in React memory and were lost on restart, and preferences sat in `localStorage`. moby-api is stateless, so the app has to keep any history itself. The later stages (tools, approvals, reminders, the robot link) also need local tables, so the schema is created in full now.

## Decisions

- **SQLite, owned by Rust.** One file, `moby.db`, in the app data folder (`%APPDATA%\uz.moby.desktop`). Only `src-tauri/src/store.rs` opens it. The webview calls nine narrow commands and never sees SQL, the path, or another account's rows. No filesystem or SQL plugin is added and the capability set stays empty.
- **`rusqlite` with bundled SQLite, pinned exactly** (`=0.41.1`, with SQLite compiled in by `libsqlite3-sys`). The app then does not depend on the SQLite that happens to be on the computer, and a bundled upgrade is a reviewed change.
- **Versioned migrations.** `PRAGMA user_version` records the step. Each step runs in its own transaction, a failed step leaves the file at the previous version, and a file from a newer app is refused instead of opened. Version 1 creates `settings`, `conversations`, `messages`, `projects`, `allowed_apps`, `grants`, `reminders`, `audit_log`, and `devices`. Only the first three have behaviour. `devices` holds the id and name; the `pair_key` is not in the database.
- **Accounts.** Rows that belong to an account carry `user_id`. Rust takes it from the profile the API client received and clears it on sign-out or when the session ends, so the webview cannot ask for another account's history. Settings, retention, and delete-all are per computer. Signing out keeps the file.
- **Agent context.** The chat sends the active conversation only: at most 40 messages, each clipped to 8 KiB, and 32 KiB in total, newest first, never starting with a reply. That keeps the existing 40 message limit and stays well under the API's 64 messages and 8 KiB per text.
- **Retention.** Setting `retention_days` defaults to 30 and the person can pick 7, 30, 90, 365 days, or keep history until they delete it. Rows older than that (by last activity) go at startup and whenever the setting changes. The audit log follows the same rule.
- **Delete everything.** One command removes every conversation, message and audit row of every account, then runs `VACUUM`. Settings stay.
- **Deleted text does not linger.** The connection sets `secure_delete`, so every delete (one conversation, retention, delete-all) overwrites the freed bytes with zeros. `VACUUM` after delete-all additionally rebuilds the file so it shrinks. Journal mode stays the default rollback journal, which is removed after each commit; WAL would keep old pages in a second file until a checkpoint. Tests read the file bytes after deleting and assert that the text is gone.
- **Failures do not stop chat.** If the file cannot be opened or a write fails, chat continues from memory and the app shows a warning that nothing is being saved. Message text is never logged.
- **Settings location.** The reply language is stored in the Rust settings (`uz`, `ru`, `en`; Uzbek is Latin script) and an older `localStorage` choice is carried over once. The theme stays in `localStorage`: it is a per-viewer convenience that must apply before the first paint.
- **Input checks in Rust.** Ids must be canonical lowercase UUIDs, texts are 1 byte to 16 KiB, titles 1 to 80 characters without control characters, roles `user` or `moby`, languages one of three, retention 1 to 3650 days. All SQL is parameterized, and tests store injection strings and check they come back unchanged.

## Alternatives

| Option | Why not |
| --- | --- |
| Files or JSON in the webview (`localStorage`, IndexedDB) | The webview would own the data, there are no migrations or per-account scoping, and other code on the page could read it. |
| A JSON file written by Rust | Rewrites the whole file on each message, no transactions, and every later table would be hand-built. |
| `tauri-plugin-sql` or a filesystem plugin | Hands SQL or paths to the webview and widens the capability set. |
| System SQLite, or `sqlx` | A system library can differ per computer. `sqlx` is async and large for one local file. |
| SQLCipher (encrypted database) | Rejected for now, see below. |
| WAL journal mode | Deleted pages stay in the `-wal` file until a checkpoint, which weakens the delete guarantee for no benefit at this size. |

## Disk protection

Decided 2026-11-04 by the project owner (open decision 9 in the plan): the file relies on **OS user-profile protection** only. It sits in the person's Windows profile, so other standard Windows users cannot read it, and BitLocker or device encryption covers a lost computer. SQLCipher is **rejected for now**: it needs a key, and the only place to keep one is the same Windows account that already protects the file, so it would add a build dependency without a stronger boundary against someone using that account. Revisit if the app gains a separate unlock step. The settings page says this plainly.

## Owner confirmations (2026-11-04)

- History retention defaults to **30 days** and stays configurable.
- Disk protection is OS user-profile protection, with SQLCipher rejected for now.
- Interface languages are Uzbek (Latin), Russian and English. Translating the interface is **not** part of D2. The setting stores those three values, interface text is kept in the screen components, and the date locale is one constant, so translation can be added without reshaping the data.

## Evidence

- `cargo test --locked` in `src-tauri` covers migrations (fresh, incremental, rolled back, newer file refused), CRUD, account isolation, retention at startup and on change, delete-all, hostile strings stored literally, oversize and malformed input, a store that cannot open, and file-level checks that deleted text is gone.
- `npm test` covers the agent context limits and titles.

## Open

1. **Account deletion.** Deleting the account on the server does not delete that account's local history. Retention or **Delete all history** removes it. Decide whether account deletion should offer to remove it too.
2. **Roaming.** `%APPDATA%` is the roaming profile. On a domain with profile roaming the history file would travel with it, so `%LOCALAPPDATA%` may suit better.
3. **Activity log.** The table exists and retention and delete-all cover it, but nothing writes to it and its clear button is a placeholder until tools arrive.

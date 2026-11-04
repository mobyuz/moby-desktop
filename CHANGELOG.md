# Changelog

## Unreleased

- Safe local tools: Moby can read a registered project's Git status (in process, without starting `git`), search its file names, create reminders with a Windows notification, and, after you confirm on screen, open a program or a project. The agent loop moved into Rust, so the interface can only approve or deny a dialog Rust opened.
- **Tools and projects** and **Activity** pages, an approval dialog that starts on Deny, read permissions you can revoke, and a working activity log that stores decisions only.
- Windows path checks before every use of a registered folder or program, and a test corpus with real junctions.
- History now lives in `%LOCALAPPDATA%` (existing files move there once). Deleting your account deletes its local data too, and Delete all history says it covers every account on this Windows user.

- Tauri 2 shell with a strict CSP, an empty capability set, and single-instance startup.
- Virtual Moby face that draws and animates all nine protocol expressions.
- Windows CI that checks the frontend and Rust code and builds a per-user NSIS installer.
- Rust client for moby-api. Tokens stay in the Windows Credential Manager and Rust memory, so you stay signed in between launches.
- Sign-in, registration, and password reset screens, a chat that streams replies, and an account page for your name, usage, signed-in computers, data export, and account deletion.
- One-column layout built around the chat, with a black and white design, the bundled Onest typeface, and light, dark, and system themes. Moby's eyes take a colour from its mood.
- Local history. Conversations are saved per account in a SQLite file in your Windows profile and survive restarts, with a conversation list to start, switch and delete them. If the file can't be used, chat keeps working from memory with a warning.
- Settings page that also works before you sign in: appearance, reply language (kept on this computer), how long to keep history (30 days by default), and delete all history. Signing out keeps your history.
- The database schema for later stages (projects, allowed apps, grants, reminders, activity log, devices) is created up front through versioned migrations.

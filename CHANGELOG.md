# Changelog

## Unreleased

- Security review fixes for the robot link and voice: a robot's `pair_reset` no longer deletes the saved key (you choose to unpair); turning cloud voice on and opening a USB port that does not look like a Moby are confirmed in a native window; hellos are rate-limited; robot text loses bidirectional and zero-width characters; one voice stream is capped at 30 s; revoking voice stops the turn first; and `scripts/check-release-excludes-dev.ps1` checks the release binary for the simulator and the PC microphone.

- Voice chain (stage D4): hold Moby's talk button, and the app checks the audio frame by frame, sends it to the speech service only after you turned voice on, runs the usual agent turn (tools and confirmations unchanged), and reads the answer on this computer's speakers. Voice is off until you accept, you can turn it off at once, and Stop cancels any stage. A lost frame, an overrun, a cut-off stream or the mute switch ends the turn with a clear message. Audio and spoken words are never logged or written to disk. Tested against a simulator and a stub server; **not tried with a real robot, speech service or sound device**; latency and speech quality are unmeasured. Development builds also have a PC-microphone path with a visible "MIC ON" bar; release builds do not contain it. See [ADR 0007](docs/adr/0007-voice-chain.md).

- Robot link over USB (stage D3): connect to Moby, answer its hello, keep the session alive, pair with the talk button, and show connected, muted, pairing and error states with the face following the robot. Tested against a built-in simulator; **not yet tried on a real robot**. A **Robot** page shows status, pairing and paired robots, and in development builds a panel drives the simulator.
- Safe local tools: Moby can read a registered project's Git status (in process, without starting `git`), search its file names, create reminders with a Windows notification, and, after you confirm on screen, open a program or a project. The agent loop moved into Rust, so the interface can only approve or deny a dialog Rust opened.
- **Tools and projects** and **Activity** pages, an in-page dialog for reminders that starts on Deny, read permissions you can revoke, and a working activity log that stores decisions only.
- Every program or project launch is confirmed on its own, in a separate Windows window that the chat page cannot click (OK / Cancel, Cancel is the default). There is no "always" or "this session" for launches. Clearing the activity log is confirmed the same way. (Owner decision, 2026-11-05.)
- `git_status` refuses repositories that point at other folders or network shares (`commondir`, `alternates`, `gitdir`) and links inside `.git`, and stops on cancel and timeout. Shells, script hosts and launchers cannot be added as programs. The page sees only the last two parts of a folder's location.
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

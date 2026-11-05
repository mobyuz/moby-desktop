# 0006 Robot link over USB, with a simulator (D3)

Status: accepted for the host side. Nothing here has run against a real robot. Date: 2026-11-05.

## Context

The desktop has to find a Moby on a USB serial port, run the session the firmware expects, pair with it, and show its state. No robot exists yet, so the test target is a simulator that follows the firmware (`moby_session.c` and ADR 0001 on `claude/f3-usb-link` in moby-firmware). The protocol is `moby-protocol` 0.2.0, read from the sibling checkout as in [ADR 0001](0001-desktop-foundation.md); its codec does all framing and message parsing.

## Choice

- **One transport trait** (`Transport`: `read` with a wait, `write_all`). Two implementations: a USB serial port (`serialport`) and the simulator. The session, pairing and reconnect code is the same for both.
- **Layers.** `link.rs` turns bytes into robot control messages with `FrameDecoder` and counts everything it drops. `session.rs` is a state machine with no I/O (handshake, keepalive, pairing, state). `supervisor.rs` finds the port, reconnects and publishes. `service.rs` is the thread and the IPC handle. Time comes from a `Clock`, so tests run a minute of robot time in milliseconds on a clock they move by hand.
- **Serial crate:** `serialport =4.10.1`, no default features (libudev is Linux only). It lists ports with USB vendor and product IDs on Windows and times out a read as `TimedOut`. Its licence is MPL-2.0, file-level copyleft, compatible with GPL-3.0-only. The port is opened with `dtr_on_open(false)`, because some DTR/RTS sequences reset an ESP32; whether the robot needs DTR is a board question.
- **Port detection:** by USB VID/PID `303A:1001` (Espressif USB Serial/JTAG). Many Espressif boards share it, so it only marks a port "looks like Moby"; the hello proves it. A port the webview names is opened only if it is in a fresh listing of USB serial ports. Nothing opens by itself unless a robot has been paired on this computer; then one automatic connection attempt is made, and only if exactly one port looks like a Moby. With two candidates the app asks.
- **Keepalive (firmware requirement, now also in `docs/protocol.md`):** the robot ends the session after three heartbeats with no valid desktop frame, about 8 s. The desktop sends a frame at least every 1 s when nothing else is due: an empty `set_config`, whose `command_ok` is ignored. Robot heartbeats missing for 6.5 s end the link; the supervisor reconnects after 0.5, 1, 2, 4, 8, then every 10 s, and the wait resets after a session is established.
- **Compatibility:** `hello_ack` is accepted only for protocol version 1. Another version gets `accepted: false` and a message naming both versions. Required capabilities are an explicit list, empty in this stage (voice is D4 and will require `usb_pcm`); unknown capabilities are ignored.
- **Hardware mute is authoritative.** The desktop only reads it (hello, `mute_changed`, heartbeats). No message it builds touches mute. Muted outranks every other state in the face and the status chip, and pairing is refused while it is on.
- **Pairing:** `pair_begin` (30 s) → robot `command_ok` → talk button → `pair_button_confirmed` → the desktop makes 32 bytes with the operating system generator (`getrandom`, `BCryptGenRandom`), **saves the key and the device first**, then sends `pair_set_key`. A robot that refuses gets the saved key taken back; a lost reply keeps it, because the robot may have stored it (the next hello then proves it). A confirmation without a pairing in progress is ignored and counted. `pair_reset` makes the desktop forget key and record (protocol step 7). The key crosses IPC never: the page calls `robot_begin_pairing` with no arguments and sees only states and short messages. It is in Credential Manager (entry `robot-key-<device id>`, via the existing vault in `api.rs`) and in the robot, and `PairKey` prints as redacted. SQLite `devices` holds only ID and name.
- **Scope of pairings: per Windows user, not per Moby account.** The robot is plugged into this computer, Credential Manager is per Windows user, and the history file is too. Signing out, switching accounts or deleting an account does not remove a pairing; Unpair does, and so does the robot's own reset.
- **Audio is out of scope.** `audio_start` and `audio_end` events and robot audio frames are counted and dropped; the desktop never starts anything.
- **IPC:** nine narrow commands, inputs validated in Rust, state pushed through a `Channel` the page passes in (so no event permission is needed). The capability set stays empty and no plugin is added.
- **Simulator:** `sim.rs`, compiled only for tests and builds with debug assertions. It does the handshake with proof checking, heartbeats, the 8 s liveness rule, pairing with a simulated button, mute, state changes, errors and rate limits, and has fault injection (corrupt frames, oversized segment, boot-text noise, silence, pulled cable, wrong protocol version). One IPC command, `robot_dev`, drives it; in a release build the command exists only as a stub that answers `not_available`. The panel is behind `import.meta.env.DEV`, so the production bundle does not contain it (checked by searching `dist`).

## Alternatives

| Option | Why not |
| --- | --- |
| Hand-written framing | The protocol crate has the codec and the vectors. |
| Async serial (tokio-serial) | One short-lived blocking thread is simpler, and the session stays testable without a runtime. |
| Open any port the page names | Lets a page open arbitrary devices. |
| Auto-connect to any 303A:1001 port | Could open and reset someone's other ESP32. |
| Send `pair_set_key` first, save after | A crash between them leaves a robot holding a key nobody has. |
| Pairings per Moby account | The USB robot is tied to the computer; signing out would orphan the key. |
| `listen()` events | Needs `core:event` permission in the webview. |

## Evidence

`cargo test --locked` runs 51 robot tests against the simulator. They cover: handshake with and without a key, wrong key, paired elsewhere, unreadable vault; keepalive (a frame at least every 2 s over 30 s, no drop), and the simulator itself dropping a silent desktop after about 8 s; heartbeat loss and reconnect, backoff 0.5 to 10 s and its reset, pulled cable; corrupt CRC, oversized segment, ROM text, malformed JSON, unknown fields and names, wrong sender, audio; version mismatch and missing capabilities; pairing happy path, button timeout, wrong order, already paired, mute during pairing, key not saved, refused key, `pair_reset`, unpair; the key absent from snapshots, logs and `Debug`; and the shared protocol vectors (`usb-vectors.tsv`, `control-vectors.tsv`) through the desktop's link layer. `npm test` covers the status and face rules and that the page has no key path.

## What still needs a board

Everything about real USB is unverified: that Windows lists the robot as `303A:1001` and the port opens; hello delivery after the port opens; whether `dtr_on_open(false)` is right (the robot may need DTR, or a DTR/RTS sequence may reset it); behaviour when the cable is pulled mid-frame; write timeouts; the ROM boot text on connect; throughput (D4); the 10 s long press; key survival in NVS across resets; and a real session with the real firmware. The key crosses the cable in clear text and is stored unencrypted in the robot, as the protocol decision records. A pairing cancelled in the app cannot be cancelled on the robot, which keeps waiting for its own timeout.

## Decisions to confirm

- Pairings per Windows user (above).
- The desktop does not delete its key when a robot merely reports `paired: false` in a hello (a spoofed device could cause that); only `pair_reset` does.
- Auto-connect at start only when a robot was paired before and exactly one candidate port exists.

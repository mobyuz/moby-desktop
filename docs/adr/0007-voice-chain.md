# 0007 Voice chain: robot push-to-talk, consent, speech to text, agent, text to speech, playback (D4)

Status: accepted for the desktop side. Everything runs against a simulator and a stub server. **Nothing here has run with a real robot, a real speech provider, or a real sound device.** Date: 2026-11-05.

## Context

The robot's firmware (`claude/f4-ptt-audio`, ADR 0002 there) streams the microphone while the talk button is held: `audio_start`, frames of 640 bytes (20 ms, 16 kHz mono `pcm_s16le`, sequence from 0 per stream), `audio_end` with a reason. `moby-api` takes a whole WAV (mono 16 kHz 16-bit, up to 30 s) at `POST /v1/voice/stt`, answers a transcript with language, script and confidence, and turns up to 500 characters into the same WAV at `POST /v1/voice/tts`. Both answer `403 voice_consent_required` unless the account accepted the current `cloud_voice` version (`GET/POST /v1/consents`). Owner decisions: the product hears through the robot's microphone only; the PC microphone is a development tool; interface languages are Uzbek Latin, Russian and English.

## Choice

- **Stream assembly in the link (`robot/audio.rs`).** A strict state machine in front of the session: one stream at a time; the stream ID, sequence number (next expected, from 0), and frame size must match every frame; a total of 20 s / 640 000 bytes at most (the firmware stops at 15 s, the API takes 30 s); no frame for 1.5 s drops the stream; mute (from `mute_changed`, a heartbeat, or `audio_end` reason `mute`), `E_AUDIO_OVERRUN`, a robot `cancel`, a wrong frame count, an empty or under 200 ms stream, a second `audio_start`, a start while not authenticated or muted or without `usb_pcm`, a `wake` start, or any format other than 16 kHz mono all end the turn with a named reason. Partial audio is never passed on. A desktop `cancel` command tells the robot to stop whenever the desktop ends a stream. The buffer is zeroed on every ending (`Pcm` zeroes on drop and prints only its length). D3's "count and drop" for audio is gone.
- **`usb_pcm` is not a required capability.** ADR 0006 said D4 would require it. The firmware's default image has no audio, and requiring it would make that robot incompatible even for pairing. A robot without it still pairs; the interface says it has no voice link. To confirm.
- **One turn is one future (`voice/mod.rs`).** Consent, STT, hand-off, agent turn, TTS, playback. Stop, a new button press, revoking consent and sign-out cancel it; the STT, TTS and agent requests are dropped with it, and playback is stopped. The agent stage is ended through the agent's own cancel, so its dialogs close and its audit entries are written as before.
- **The agent turn is unchanged.** The transcript goes to the page (so it is shown and saved in the conversation under the existing retention), and the page continues with `voice_turn`; Rust refuses unless the newest message is exactly the transcript it holds. The language is the one the speaker was heard in. Tools, approvals, native launch confirmation and limits are the existing code. The voice module contains no path to answer an approval (a test reads its source), so voice cannot approve anything.
- **Consent.** Off until accepted. Turning it on is confirmed in a native window that Rust opens (the existing `Confirmer`), saying that audio leaves the computer, which server steps follow, the consent version and any text the server sends (`GET /v1/consents` returns versions only today); a compromised page cannot turn it on by calling the command. Withdrawing needs no window. Accepting reads the current `cloud_voice` version, posts `accepted: true` for it, and only then records account and version in the local `settings` row (migration 0003). Another account on the same computer starts off. Revoking turns it off locally first, cancels the running turn, then tells the server (a failure there is reported but local voice stays off). The consent is checked before upload and again before the answer is spoken; a 403 from the server clears it. A stream that starts without consent is cancelled at the robot.
- **Playback** uses `rodio =0.23.0` (cpal 0.18.2 underneath), playback and recording features only, no decoders; the player runs on its own thread and the sound device never crosses into async code. The webview gets no capability. Output goes through a `Speaker` trait, so tests use a fake. Robot-side playback (`audio_play_start`) is left for later: the firmware offers no speaker capability yet.
- **The PC microphone** (`voice/mic.rs`) is compiled only where the simulator is (`cfg(any(test, debug_assertions))`). It starts only on request in the development panel, needs consent, and the interface shows a red "MIC ON" bar for exactly as long as it captures. A compile-time assertion and a marker string (`MIC_MARKER`) prove it is absent from a release build, the same way as the simulator (ADR 0006). The `recording` feature of `rodio` is still compiled (features are not `cfg`), but nothing in a release build uses it.
- **Faces.** Virtual: listening, thinking, speaking, error follow the voice stage; the robot's hardware mute still wins. Robot: `thinking` and `speaking` are sent while the chain works, then the answer's `expression_hint`, `error` on failure, `confused` for an unsure transcript (confidence under 0.35, an unmeasured proposal from the plan). `listening` is the robot's own.
- **Review fixes.** A stream also has a 30 s wall-clock limit (a frame a second never stalls and never fills the buffer). `tokio::select!` is unbiased, so every request is now guarded by an explicit `is_canceled()` check and a `biased` select with cancel first. Revoking sets an in-memory flag and stops the turn before the settings write that can fail, so voice stays off for the run even when the file cannot be written. The compile-time `assert!` guards on the release build were tautologies (they repeated the `cfg`) and are gone; `scripts/check-release-excludes-dev.ps1` searches the built binary for the marker strings instead, and CI runs it.
- **A new button press cancels the running turn** (plan: "Bekor qilish"). The firmware refuses the button while thinking or speaking, so this only matters for the simulator and future firmware.
- **Speech text** longer than 500 characters is cut after the last sentence end that fits.
- **Privacy.** `Voice::log` takes only `&'static str`, so audio and text cannot be logged. The audit log stores decisions only, as before. Audio and the WAV are in memory only, zeroed by us at the end of the turn; the HTTP library's internal copies are freed, not zeroed. No wake word, no always-listening mode.

## Alternatives

| Option | Why not |
| --- | --- |
| Webview records or plays audio | Needs microphone permission in the webview and sends audio through the page; the plan keeps audio in Rust. |
| Run the whole voice turn in Rust, with its own history | Duplicates the conversation store and its retention. The page already owns it. |
| Require `usb_pcm` | See above. |
| Pass partial audio when a frame is lost | The plan says the stream is marked with an error; a gap makes the transcript unreliable. |
| `cpal` directly | `rodio` adds nothing but a mixer for playback and a tested microphone wrapper; both are small. |
| Streaming STT or TTS | Not offered by the API; the plan says add it only if measurement needs it. |
| PC microphone in the product | Owner decision: no robot-less voice mode. |

## Evidence

`cargo test --locked`: the stream assembler (12 unit tests: whole stream, gaps, repeats, swaps, size, hard bound, count, mute, cancel, overrun, stall, odd starts, link drop); the simulator through the supervisor (12 stream faults each ending with its own reason and followed by a healthy turn, mute with a robot that keeps sending, Stop, face commands that obey mute, a refused start); 34 voice tests against a stub server (happy path with every stage and face, no consent means no request, another account, accept and revoke with versions, revoke during STT drops the request, cancel at each of listening, STT, hand-off, agent, TTS and playback, STT and TTS error statuses and timeouts, an unsure transcript, a page that never continues, a page that changes the words, a spoken yes that approves neither a reminder nor a program launch, no audio or transcript in logs, audit or any file, no microphone without consent); one end-to-end test from the simulated robot through the stubs. `npm test` covers the face mapping, the privacy text, and that the page has no audio API and no capability.

## Audio needs a proven robot (D4 review)

`audio_allowed()` is true only after the robot proved it holds the pairing key (ADR 0006). Before that, or after a failed proof, an `audio_start` is refused with `audio_not_allowed` and cancelled, frames are counted as ignored, and the voice chain never sees an utterance, so no speech request, agent turn or playback can follow. The proof is that a device with the real ID, no key, that answers keepalives with `command_ok` and streams audio produces zero requests at the stub server (`a_forged_device_with_the_real_id...`).

## What the tests cannot prove

The native confirmation windows (consent, opening an unknown port) are never shown by tests; they run a scripted confirmer, so only their text and the refusal path are checked. `tripwire_the_voice_module_does_not_mention_approval_calls` reads one file and is a tripwire; the proof that voice cannot approve is the two behavioural tests. Playback and the microphone never touch a device in tests.

## What needs real hardware or a real provider

Real USB throughput for 32 KB/s of audio next to control traffic; the 15 s limit and 1.5 s stall limit against real timing; the F4 firmware itself; real speech quality in Uzbek, Russian and English (including Latin and Cyrillic and mixed language); the confidence threshold; latency (nothing measured, the plan's targets are untested); `rodio` on real Windows devices (no device was opened by any test); the PC microphone on a real device; the `Hang` cases against the real API's 15 s provider deadline.

## Decisions to confirm

- `usb_pcm` not required for connecting (above).
- The 20 s hard bound and 1.5 s stall limit; 0.35 confidence.
- A new press cancels the running turn.
- A short AI-voice note is shown next to the voice controls although the plan names no such requirement.
- Consent is per account and per computer.

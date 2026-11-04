# Moby Desktop

The Windows desktop companion for [Moby](https://github.com/mobyuz), a voice-enabled desk robot. It is built with Tauri 2 (Rust), React, TypeScript, and Vite.

The app talks to the Moby robot over USB, to the Moby backend over HTTPS, and runs the actions you approve on your own computer. Conversation history stays on your computer.

## Status

Early development. The app currently runs in **demo mode**: the sign-in, chat, usage, consent, and device screens use an in-memory stand-in for the backend, and the robot is simulated. No server is contacted and nothing leaves your computer. Wiring the real backend, the USB link, and real tools comes next.

| Area | State |
| --- | --- |
| App shell, virtual Moby face with nine animated expressions | Done |
| Sign-in, profile, usage, installations, consent, tools, approvals, devices, settings, diagnostics screens | Done, on demo data |
| Real backend client and token storage | Planned |
| USB robot link and audio | Planned |
| Safe local tools (`git_status`, `open_app`, and others) | Planned |
| Local history database, signed updates, installer distribution | Planned |

## Try the demo

Sign in with the demo account (the sign-in screen has a button that fills it in), or continue without an account. Ask Moby for the git status on the Home screen, approve the request, and watch the face and the usage meters change. The result is staged and no command is run.

## Building

You need Windows 11, Git, Rustup (the toolchain is pinned in `rust-toolchain.toml`), the Visual Studio C++ Build Tools, and Node 24.13.0 with npm 11.6.2. WebView2 ships with current Windows 11.

This app depends on `@mobyuz/moby-protocol`, which is not published yet. For now it is read from a sibling checkout, and **that repository is currently private**, so outside contributors cannot build the app until the protocol is released. This will be replaced by a pinned package version once the first protocol release exists (see [ADR 0001](docs/adr/0001-desktop-foundation.md)).

With access to both repositories:

```powershell
Set-Location D:\repos\moby
gh repo clone mobyuz/moby-protocol
Set-Location .\moby-protocol
npm.cmd ci --ignore-scripts --no-audit --no-fund
npm.cmd run build
Set-Location ..
gh repo clone mobyuz/moby-desktop
Set-Location .\moby-desktop
npm.cmd ci --ignore-scripts --no-audit --no-fund
```

`npm.cmd` avoids the PowerShell execution-policy restriction on `npm.ps1`.

## Run and check

```powershell
npm.cmd run tauri -- dev
npm.cmd run lint
npm.cmd run typecheck
npm.cmd test
Set-Location src-tauri
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
```

Build the app, or the per-user installer, from the repository root:

```powershell
npm.cmd run tauri -- build --no-bundle
npm.cmd run tauri -- build --bundles nsis
```

The binary is `src-tauri/target/release/moby-desktop.exe` and the installer goes to `src-tauri/target/release/bundle/nsis/`. A release build compiles a large Windows crate and can use several gigabytes of memory. If the compiler crashes, close other applications or limit parallelism with `CARGO_BUILD_JOBS=1`.

## Layout

| Path | Purpose |
| --- | --- |
| `src/face/` | The virtual face and per-expression metadata. |
| `src/screens/` | One file per screen. |
| `src/store.tsx` | App state and the scripted demo agent. |
| `src/demo.ts` | In-memory stand-in for the backend, shaped like the Moby API. |
| `src-tauri/` | Rust shell, window, CSP, and capabilities. |
| `tests/` | Unit tests. |
| `docs/adr/` | Decision records. |

## Security

The webview has no filesystem or shell access and an empty capability set. The production content security policy allows only the app's own files. Secrets such as refresh tokens will live in the Windows Credential Manager, never in the web layer. Please do not put real credentials in issues or pull requests.

## Contributing

Be kind. This project follows the [Code of Conduct](CODE_OF_CONDUCT.md).

## License

Copyright (c) 2026 Moby contributors. Released under the [GPL-3.0-only](LICENSE) license.

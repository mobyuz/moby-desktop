# 0001 Desktop foundation (D0)

Status: accepted for D0, two items open below.

## Decisions

- **Stack.** Tauri 2 (Rust) with React, TypeScript, and Vite, Windows only, as in the v1.0 plan. Dependencies are pinned to exact versions and match `moby-web` where they overlap.
- **Webview surface.** `capabilities/default.json` has no permissions and no shell or filesystem plugin is enabled. The production CSP allows only `self`. `devCsp` is looser because the Vite dev server injects inline styles. The only plugin is `single-instance`, which has no webview commands.
- **Installer.** NSIS in `currentUser` mode, so no administrator rights are needed. Code signing is the open decision 7 in the plan and is not part of D0.
- **Face.** The face is inline SVG driven by `data-expression` and `data-animation`. All motion is CSS, so no inline `style` attributes are needed under the strict CSP, and `prefers-reduced-motion` stops the animation while keeping each expression visually distinct.

## Open

1. **`moby-ui`.** The plan puts the face in a shared `moby-ui` package. That repository does not exist yet, so the face lives in `src/face/` and is written to move out unchanged. It must not be copied into `moby-web`. (Done: see [ADR 0008](0008-moby-ui.md).)
2. **`moby-protocol` source.** No tag or package has been published. `package.json` uses `file:../moby-protocol` and CI builds the protocol from `main` into a sibling folder. Replace both with a pinned `@mobyuz/moby-protocol` version after the first release, and add the Rust crate by git tag when the first Rust module needs it.

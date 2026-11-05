# 0008 The face comes from moby-ui

Date: 2026-11-05
Status: accepted. One item open below.

## Context

ADR 0001 kept the face in `src/face/`, "written to move out unchanged", because the shared `moby-ui` package did not exist. It does now (`@mobyuz/moby-ui` 0.1.0, unpublished; like every Moby repository it is to be public) and exports `MobyFace`, `EXPRESSION_META`, and two stylesheets.

## Choice

1. **Use `MobyFace` everywhere and delete `src/face/`.** The screens that drew the face (starting, offline, sign-in, chat, robot) import it from the package. The old component, its CSS (about 340 lines), and its test are gone. The package has its own tests for the expression list and the motion, including the drift check against `moby-protocol`.
2. **Sibling dependency until a release.** `package.json` has `"@mobyuz/moby-ui": "file:../moby-ui"`, the same temporary approach as `moby-protocol` (ADR 0001). From a checkout named `moby-desktop` or a worktree such as `moby-desktop-d3` next to `moby-ui`, the path is `D:\repos\moby\moby-ui`. The package's `exports` point into `dist/`, so **moby-ui must be built first** (`npm ci --ignore-scripts; npm run build` in `moby-ui`, and the same for the protocol). **After moby-ui has a release, this becomes a pinned version** (`@mobyuz/moby-ui` from the registry) and the CI checkout goes away.
3. **Two stylesheets, imported once in `main.tsx`.** `@mobyuz/moby-ui/styles.css` styles the face. `@mobyuz/moby-ui/mood.css` (added to moby-ui for this) gives any `[data-expression]` element `--moby-mood`, which the page background glows in `app.css` read instead of the old `--mood`. The colours are the same ones as before. Neither file adds inline styles, `@import`, `url()`, or fonts.
4. **Looks kept with three rules in `app.css`.** `--moby-face-bezel: var(--line)` keeps the app's outline colour (the package's own default is a little darker in light mode). `.moby-face[data-theme="auto"] { color-scheme: inherit }` makes the package's `light-dark()` follow the app's own light or dark setting, not only the system's. The "?" mark keeps the app font. The hero face transition also keeps the 0.5 s colour fade.
5. **Unchanged:** the expression flow from the robot and voice (`app.expression` into `data-expression` and the `MobyFace` prop), the status text from `EXPRESSION_META`, the aria-labels ("Moby is thinking"), and reduced motion (the package's `data-motion="auto"` rule for the face, the app's existing global rule for everything else).
6. **CSP stays strict.** The package draws inline SVG and CSS only, with no inline `style` attributes, no `data:` and no external requests. The production CSP in `tauri.conf.json` was not touched. In the built bundle the only `https://` strings are the SVG namespace and a React error-message link, neither of which is requested.

## Open

- **CI** checks out `moby-ui` next to the app (see `.github/workflows/ci.yml`). All repositories are public (owner decision), so the checkout needs no token and the `MOBY_UI_READ_TOKEN` secret is gone. Pending: `moby-ui` has no remote yet, so the job fails at checkout until it is pushed to GitHub.
- `moby-ui` and the desktop each keep their own copy of `react` in `node_modules`, because `moby-ui` is linked from outside. The face uses only the JSX runtime, so this is harmless; deduplicate in the Vite config if a hook-based component is ever added to the package.

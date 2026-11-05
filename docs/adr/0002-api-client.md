# 0002 API client in Rust

Status: accepted.

## Decisions

- **Rust owns the network and the tokens.** The webview calls commands in `src-tauri/src/api.rs` and only sees account data. The refresh token is in the Windows Credential Manager and the access token in memory. moby-api has no CORS layer, and the CSP stays limited to `self`.
- **Commands, not a proxy.** Each API call the app needs has its own command, so the webview cannot reach any other endpoint.
- **Refreshes run one at a time.** moby-api revokes the whole session when a refresh token is reused, so a lock covers each refresh.
- **Show only what works.** The demo backend and the screens for robots, local tools, approvals, and diagnostics are gone. They return when each feature works end to end.

## Open

1. **Production address.** `MOBY_API_URL` has no public default yet.
2. **Policy versions.** Registration needs the server's terms and privacy versions, and no public endpoint returns them. They are set at build time and default to `dev-1`.

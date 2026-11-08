# MixchatDesktop: AI Agent Guide

## Overview

**MixchatDesktop** is a Tauri 2 desktop shell for Mixchat. A Rust shell (`src-tauri/`) spawns the Mixchat Astro server as a bundled Node sidecar (`mixchat-node`), waits for it to answer on `http://localhost:3000`, and opens the UI in a WebView2 window. The Mixchat source is consumed from the sibling checkout `../Mixchat` (https://github.com/grepfs17/Mixchat) and is not forked here. Windows-only distribution: NSIS installer and a portable zip.
- Build, run, and configuration details in README.md
- Threat model and what ships inside binaries in SECURITY.md

## Operating principles
- Working code only. Plausibility is not correctness. Verify before reporting done.
- Never fabricate file paths, APIs, commit hashes, command output, or test results. Read the file, run the command, or say what is unknown.
- Say when a premise appears wrong before implementing around it.
- Ask before proceeding only when a request has multiple plausible interpretations and the choice materially affects the result.
- Touch only what the task requires. Avoid drive-by refactors, formatting, or cleanup.
- Keep communication direct and concise. Skip flattery, filler, ceremonial openings, and emoji.
- Avoid em dashes (U+2014) and semicolons in comments, prose, and user-facing text. Use commas, colons, periods, or parentheses instead.
- Add comments only when necessary. Keep them concise and easy to read, and explain non-obvious intent instead of restating the code.

## Command execution
- Use always pnpm, avoid npm commands. In case of using npx use pnpx or pnpm dlx instead.
- Prefer running code, tests, and checks over guessing.
- Read complete errors, logs, and stack traces before fixing them.
- For Rust changes run `cargo check` and `cargo test` inside `src-tauri/` before reporting done.
- The secret guard (`scripts/check-secrets.mjs`) runs in every packaging path and must pass. It never prints secret values, keep it that way.
- Never spawn `pnpm.cmd` (or any `.cmd`/`.bat`) directly from Node `child_process` without going through `scripts/pnpm.mjs`. Modern Node refuses that spawn (CVE-2024-27980, `spawn EINVAL`) and the failure is silent.
- Update README.md when scripts, commands, or the architecture described there change.

## Environment Variables

Shell runtime keys (read by `src-tauri/src/config.rs`, forwarded to the sidecar only if allowlisted):

| Variable | Purpose |
|----------|---------|
| `MIXCHAT_PORT` | Local server port (default `3000`, must match OAuth redirects) |
| `MIXCHAT_HOST` | Server listen address (default `127.0.0.1`, forced loopback when packaged) |
| `MIXCHAT_URL` | Origin the window loads (default `http://localhost:<port>`) |
| `MIXCHAT_DIR` | Mixchat checkout if not `../Mixchat` |
| `MIXCHAT_CONFIG` | Explicit desktop `.env` file |
| `MIXCHAT_MODE` | `dev`, `local`, or `packaged` (normally inferred) |

Build-time keys (`.env.desktop`, see `.env.desktop.example`, baked into the Mixchat bundle by `pnpm server:build`):

| Variable | Purpose |
|----------|---------|
| `PUBLIC_APP_PLATFORM` | Set to `desktop` by the build when `.env.desktop` exists |
| `PUBLIC_TWITCH_CLIENT_ID` | Public Twitch app client ID (Device Code Grant) |
| `PUBLIC_YOUTUBE_CLIENT_ID` | Google OAuth client ID (type Desktop app) |
| `YOUTUBE_CLIENT_SECRET` | Google Desktop-app client secret (non-confidential, baked) |
| `PUBLIC_TWITCH_ONLY` | Set to `true` to disable all Google/YouTube integrations |

## Commands

| Command | Purpose |
|---------|---------|
| `pnpm dev` | Tauri dev mode against an external Mixchat dev server |
| `pnpm start` | Run the shell in local mode against the prepared bundle |
| `pnpm server:build` | Build the Mixchat bundle and prepare it (path patching) |
| `pnpm tauri:prepare` | Full prep: server bundle, resources, jsdom, Node sidecar |
| `pnpm tauri:build` | NSIS installer (Windows host only) |
| `pnpm tauri:build:portable` | Portable zip for the host platform |
| `pnpm tauri:build:portable:win` | Cross-build the Windows portable zip from Linux (Docker) |
| `pnpm release` | Prepare once, build installer and portable, verify artifacts |
| `pnpm check:secrets` | Run the secret guard on demand |
| `cargo check` / `cargo test` | Verify and test the Rust shell (inside `src-tauri/`) |

## Project structure
- `scripts/` - build pipeline (Mixchat bundle, path patching, resources, Node sidecar fetch, secret guard, portable packaging, release orchestration)
- `src-tauri/src/lib.rs` - window creation, navigation policy, initialization script (right-click menu and keyboard shortcuts bridge)
- `src-tauri/src/server.rs` - sidecar spawn, health check, restart loop, shutdown
- `src-tauri/src/config.rs` - env loading, sidecar env allowlist, packaged-mode hardening
- `src-tauri/src/menu.rs` - context menu items and the `show_context_menu` / `menu_action` commands
- `src-tauri/src/logger.rs` - sidecar log capture with redaction
- `src-tauri/capabilities/default.json` - intentionally `permissions: []`, the webview gets no core or plugin IPC, only the app's own menu commands (allowed by default for local origins)
- `.github/workflows/release.yml` - Windows x64 CI (installer, portable, checksums, tag releases)

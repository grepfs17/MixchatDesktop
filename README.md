# MixchatDesktop

Tauri 2 desktop shell for Mixchat on Windows. It runs Mixchat's
Astro Node server as a bundled sidecar and opens the UI in the system WebView2
runtime.

The [Mixchat](https://github.com/grepfs17/Mixchat) source is consumed as a
sibling checkout (`../Mixchat`) and is not forked here.

## Requirements

- Windows 10 or 11
- Node.js LTS and pnpm
- Rust toolchain (`rustup`, `cargo`)
- The [Mixchat](https://github.com/grepfs17/Mixchat) checkout at `../Mixchat`
- A `.env.desktop` profile with OAuth client IDs

The NSIS installer embeds the WebView2 bootstrapper, so a fresh machine can
install the runtime during setup.

## Setup

Clone Mixchat next to this repo and install its dependencies once:

```bash
git clone https://github.com/grepfs17/Mixchat.git ../Mixchat
# inside ../Mixchat
pnpm install
```

Then prepare this shell:

```bash
# inside this repo
pnpm install
cp .env.desktop.example .env.desktop
```

Fill in `.env.desktop`:

- `PUBLIC_TWITCH_CLIENT_ID`: Twitch app with Client Type = Public.
- `PUBLIC_YOUTUBE_CLIENT_ID`: Google OAuth client of type Desktop app.
- `YOUTUBE_CLIENT_SECRET`: the Desktop-app client secret. Google requires it
  even for PKCE desktop clients and treats it as non-confidential.

The redirect URIs must stay on `http://localhost:3000/auth/...` unless you also
rebuild Mixchat with matching values.

Then build the local server bundle and fetch the Node sidecar:

```bash
pnpm tauri:prepare
```

## Development

Run Mixchat's dev server in a separate terminal:

```bash
# terminal 1, inside ../Mixchat
pnpm dev
```

Then start the shell against it:

```bash
# terminal 2, inside this repo
pnpm dev
```

`pnpm dev` uses Tauri dev mode and waits for the external server at
`http://localhost:3000`.

To run against the locally prepared bundle instead:

```bash
pnpm start
```

`pnpm start` runs the shell in local mode and spawns the bundled sidecar.

## Build the Windows installer

```bash
pnpm tauri:prepare
pnpm tauri:build
```

The NSIS installer is written under
`src-tauri/target/release/bundle/nsis/`.

`tauri:prepare` runs these steps:

1. Builds Mixchat with `PUBLIC_APP_PLATFORM=desktop`.
2. Prepares and path-patches the standalone server bundle.
3. Copies the bundle to `src-tauri/resources/mixchat`.
4. Installs `jsdom` next to the bundle. It cannot be bundled into the ESM server,
   so it remains the only runtime package resolved from `node_modules`.
5. Downloads the Node binary for the target triple into `src-tauri/binaries`.

The secret guard runs before packaging and refuses to embed confidential values
from the hosted site. See [SECURITY.md](./SECURITY.md).

## Portable Windows build

```bash
pnpm tauri:build:portable
```

This builds without an installer, applies `src-tauri/tauri.portable.conf.json`,
and produces a zip at
`src-tauri/target/portable/Mixchat-<version>-win-x64-portable.zip`.

Extract the zip and run `Mixchat.exe`. Settings, sessions, chat history, logs,
and WebView data live in the `app-data` folder next to the executable. The
portable build does not write to AppData or the registry, so it can run from a
USB stick or a user folder.

`pnpm tauri:portable` rebuilds the zip from an existing release build. Do not
run the portable and installed builds at the same time because both use
`http://localhost:3000`.

The portable zip does not include the WebView2 bootstrapper. Windows 11 and
current Windows 10 installs already have the Evergreen WebView2 Runtime. If it
is missing, install it from Microsoft before running `Mixchat.exe`.

### Cross-building the Windows portable from Linux

Requires Docker and network access. This produces the same `win-x64` portable
zip using `cargo-xwin`:

```bash
pnpm tauri:build:portable:win
```

The Windows SDK, Rust toolchain, and cargo registry are cached in
`~/.cache/mixchat-tauri`. The GUI still needs WebView2, so run the extracted app
on Windows.

## How it works

- `src-tauri/src/server.rs` spawns the Node sidecar, waits for
  `/api/parent-domain`, logs stdout and stderr, restarts crashes with backoff,
  and kills the process on exit.
- The window loads `MIXCHAT_URL` (default `http://localhost:3000`). Keep the
  host and port aligned with the OAuth redirect URIs.
- Server data lives under the Tauri app data directory. Logs live under the app
  log directory and are available from **Server > Open logs**.
- The shell sets `window.__MIXCHAT_SHELL__ = 'tauri'` so the shared Mixchat
  bundle can select the desktop auth flow.
- Google sign-in opens in the system browser because Google blocks embedded
  webviews. The server stores the token exchange in a short-lived handoff
  record, and the app polls for it. The poll response sets the auth cookies in
  the app's own cookie jar.
- OAuth and Twitch activation URLs are opened with `POST /api/desktop/open` on
  the local server. The webview never navigates for those links, which avoids
  the WebView2 reload.
- The window loads `?mixchat-shell=tauri` so the shared bundle selects the
  desktop auth flow even when webview initialization scripts are skipped.

## Configuration

The shell reads these `MIXCHAT_*` keys.

| Key | Purpose |
|-----|---------|
| `MIXCHAT_PORT` | Local server port (default `3000`, must match OAuth redirects) |
| `MIXCHAT_HOST` | Server listen address (default `127.0.0.1`) |
| `MIXCHAT_URL` | Origin the window loads (default `http://localhost:<port>`) |
| `MIXCHAT_DIR` | Mixchat checkout if not `../Mixchat` |
| `MIXCHAT_CONFIG` | Explicit desktop `.env` file |
| `MIXCHAT_MODE` | `dev`, `local`, or `packaged` (normally inferred) |

A `.env` file can live in the app data directory, the app config
directory, or next to the executable (in development builds, also the
repository root and current directory). Process environment variables win over
file values. Only known-safe keys are forwarded to the spawned server.

## OAuth

| Provider | Flow | Secret in binary |
|---|---|---|
| Twitch | Device Code Grant, public client | None |
| Google/YouTube | PKCE Desktop-app client, system-browser handoff | Desktop-app client secret (non-confidential) |

The hosted website's Twitch and Google web client secrets are never embedded.
The build guard fails if they appear.

Twitch sign-in opens the device activation page in an in-app webview. That
single login also stores Twitch's cookies in the shared WebView2 profile, so the
embedded player is signed in with the same account. **Account > Log in to
Twitch player** remains only as a fallback if the player cookies are missing.

## Logs and recovery

The **Server** menu can restart the server or open the current log file. If the
server exits unexpectedly, the shell restarts it up to five times with backoff
(1, 2, 4, 8, then 16 seconds). Graceful shutdown kills the sidecar, and the
Windows job object prevents orphan processes.

## Linux

*Work in progress

## License

Licensed under AGPL-3.0, same as [Mixchat](https://github.com/grepfs17/Mixchat).
See [LICENSE](./LICENSE).

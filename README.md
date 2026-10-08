# MixchatDesktop

Electron desktop shell for [Mixchat](../Mixchat). It runs Mixchat's Astro Node
server locally in the background and opens the UI in a native window, so you get
the full app (Twitch + YouTube chat, emotes, moderation, song queue, HLS) without
a browser.

The Mixchat source is not modified — this project consumes its build output and
adds the desktop wrapper.

## Requirements

- Node.js LTS and pnpm
- The Mixchat checkout as a sibling directory: `../Mixchat`
- Linux only: a working Chromium sandbox or `MIXCHAT_NO_SANDBOX=1` (see below)

## Setup

```bash
pnpm install
```

Build the Mixchat server bundle once (also patches it for cross-platform use):

```bash
pnpm server:build     # pnpm --dir ../Mixchat build + prepare:server
```

Or, if `../Mixchat/dist` already exists, just run:

```bash
pnpm prepare:server
```

## Development

Start Mixchat's dev server in a separate terminal, then attach:

```bash
# terminal 1 (in ../Mixchat)
pnpm dev

# terminal 2 (here)
pnpm dev
```

`pnpm start` runs against the locally built bundle instead (no dev server).

## Configuration

Copy `.env.example` to `.env`. The desktop app reads `MIXCHAT_*` keys for its own
behavior and forwards the rest (`ORIGIN`, `YT_*`, storage paths) to the spawned
server.

Notable keys:

| Key | Purpose |
|-----|---------|
| `MIXCHAT_PORT` | Local server port (default `3000`; must match the OAuth redirect URIs) |
| `MIXCHAT_HOST` | Server listen address (default `127.0.0.1`) |
| `MIXCHAT_URL` | Origin the window loads (default `http://localhost:<port>`); **must** match the OAuth redirect URI host |
| `MIXCHAT_DIR` | Path to the Mixchat checkout if not `../Mixchat` |
| `MIXCHAT_ENV_FILE` | Explicit Mixchat runtime `.env` to load |
| `MIXCHAT_NO_SANDBOX` | Set to `1` on Linux to pass `--no-sandbox` to Electron |

OAuth client IDs, secrets and redirect URIs are **baked into the server bundle at
build time** from `../Mixchat/.env`, so they are not needed at runtime.

## Packaging

```bash
pnpm pack             # unpacked app in release/<platform>-unpacked
pnpm build            # full installers
```

- **Linux**: AppImage + deb (produced directly).
- **Windows**: NSIS installer + single-file portable exe. Cross-building from
  Linux requires Wine (`sudo apt install wine64`).
- **macOS**: dmg (build on macOS).

`electron-builder.yml` packages the patched bundle from `build/mixchat/dist`.
Re-run `pnpm prepare:server` (or `pnpm server:build`) whenever Mixchat's `dist`
changes, before packaging.

## How it works

- `src/main/index.mjs` spawns `build/mixchat/dist/server/entry.mjs` using
  Electron's bundled Node (`ELECTRON_RUN_AS_NODE`), waits for the port, then loads
  the UI.
- The window loads `http://localhost:<port>` (matching Mixchat's OAuth redirect
  URIs) while the server binds `127.0.0.1`. The origins must match or the
  host-scoped `oauth_state` cookie is not sent back and login fails with
  "Invalid state parameter".
- OAuth popups (Twitch/Google/YouTube) open in-app so `window.opener` callbacks
  work; other external links open in the system browser.
- Packaged builds run the server with `cwd` set to the app's `userData` dir so
  session/data writes land somewhere writable, and set
  `MIXCHAT_SONG_QUEUE_DIR` / `MIXCHAT_WHEEL_DIR` there.
- `scripts/prepare-mixchat.mjs` copies `../Mixchat/dist` and rewrites the
  absolute build-machine paths Astro bakes into `entry.mjs` (client/server dirs
  and session storage) into portable values. Without this, the same bundle fails
  on a different OS (`fileURLToPath` rejects `file:///home/...` on Windows).

## Linux sandbox note

On distros that restrict unprivileged user namespaces (e.g. Ubuntu 24.04),
Electron aborts with a `SUID sandbox` error. Either run
`MIXCHAT_NO_SANDBOX=1 pnpm dev`, or fix the helper:

```bash
sudo chown root:root node_modules/.pnpm/electron@*/node_modules/electron/dist/chrome-sandbox
sudo chmod 4755 node_modules/.pnpm/electron@*/node_modules/electron/dist/chrome-sandbox
```

## Notes

- Only Linux and Windows builds are currently exercised. macOS is configured but
  untested.
- Builds are unsigned (default Electron icon), so Windows SmartScreen will warn.

Licensed under AGPL-3.0, same as Mixchat.

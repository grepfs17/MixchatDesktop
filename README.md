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
pnpm server:build     # builds Mixchat + prepares the bundle
```

By default (`../Mixchat/.env`) the build bakes the OAuth client secrets into the
app. To build a **no-secret** app instead, create a `.env.desktop` profile (see
`.env.desktop.example`) with a Public Twitch app and a Google "Desktop app"
OAuth client. When `.env.desktop` is present, `pnpm server:build` builds in
desktop mode: Twitch uses the Device Code Grant (no secret) and Google uses
PKCE with its Desktop-app client secret.

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
pnpm dist             # build all release artifacts (Linux + Windows) and clean
pnpm build            # full installers for the host OS
pnpm release          # build for the host OS and publish to GitHub Releases
pnpm run pack         # unpacked app in release/<platform>-unpacked
pnpm pack:win         # Windows unpacked + portable zip (cross-build with Wine)
pnpm clean            # remove everything in release/ except the final artifacts
```

> Use `pnpm run pack` (not `pnpm pack`, which is pnpm's own tarball command).

`pnpm dist` leaves the shippable artifacts in `release/`:

- **Linux**: `mixchat-desktop_<version>_amd64.deb` and `Mixchat-<version>.AppImage`
- **Windows**: `Mixchat Setup <version>.exe` (NSIS installer) and
  `Mixchat-<version>-win-x64-portable.zip` (unpacked, no install; includes
  `.env.example`)

`scripts/clean-release.mjs` then deletes intermediates (`*-unpacked/`,
`builder-debug.yml`) and stale versions, keeping the artifacts above plus the
update feed files (`latest*.yml`, `*.blockmap`) that electron-updater needs.
Cross-building Windows targets from Linux requires Wine (`sudo apt install
wine64`).

`electron-builder.yml` packages the patched bundle from `build/mixchat/dist`.
Re-run `pnpm prepare:server` (or `pnpm server:build`) whenever Mixchat's `dist`
changes, before packaging.

## Auto-update

Packaged builds check for updates on launch using **electron-updater** against
this repository's GitHub Releases (`publish` in `electron-builder.yml`). When a
newer version exists the user is asked before downloading, then asked before
restarting to install. A **Help → Check for Updates…** menu item triggers a
manual check. Update failures are logged, never fatal.

To publish a release:

```bash
GH_TOKEN=<personal access token with repo scope> pnpm release
```

This builds the host OS's artifacts (including `latest*.yml` + `*.blockmap`),
creates/updates the GitHub release and uploads them. Because the app itself
embeds an OAuth client secret, **always publish an update when rotating it** —
`electron-builder.yml` guarantees the metadata is shipped so existing installs
can pick it up.

- Supported: **AppImage** (Linux) and **NSIS** (Windows).
- Not supported by electron-updater: the `.deb` package and the portable ZIP —
  those users must reinstall manually.
- Updates are code-signature verified where the platform provides it, which is
  why signing (below) matters.

## Code signing

Binaries are unsigned by default. To sign, provide credentials via the
environment — no secrets are stored in the repo.

**Windows (NSIS)** — set:

```bash
CSC_LINK=/path/to/cert.pfx        # or a base64 of the .pfx
CSC_KEY_PASSWORD=<password>
```

An EV/OV code-signing certificate avoids the SmartScreen warning. Linux builds
(AppImage/deb) carry no signature.

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

## OAuth & secrets

Two operating modes, chosen by whether `.env.desktop` exists at build time:

| | Web mode (default) | Desktop mode (`.env.desktop`) |
|---|---|---|
| Twitch login | Auth code popup (Confidential app) | Device Code Grant (Public app) |
| Google login | OAuth popup (Web client + secret) | PKCE + Desktop app client |
| Secrets in app | baked into `dist/server` | Twitch: **none**; Google: desktop client secret (non-confidential) |

Desktop mode requires:
- A Twitch app with **Client Type = Public** → `PUBLIC_TWITCH_CLIENT_ID`.
- A Google **"Desktop app"** OAuth client with the loopback redirect
  `http://localhost:3000/auth/youtube/callback` → `PUBLIC_YOUTUBE_CLIENT_ID` +
  `YOUTUBE_CLIENT_SECRET` (Google rejects PKCE without the secret).

Twitch is fully secretless via the Device Code Grant. Google does not offer a
secretless flow, so its Desktop-app client secret is embedded (Google treats
desktop-app secrets as non-confidential).

These are separate from the hosted website's confidential Twitch app / Web
Google client; the website is unaffected. The desktop window must load
`http://localhost:3000` so cookies match the redirect host.

## Security & shipping

Distributed desktop binaries are fully readable by end users, so anything baked
into the build must be treated as public. Desktop builds ship only public
client IDs plus the Google **Desktop-app** client secret, which Google classes
as non-confidential. Twitch ships no secret (Device Code Grant), and the
hosted-site secrets / YouTube proxy token are never embedded.

A build guard enforces this: `scripts/check-secrets.mjs` scans the compiled
bundle and **fails the build** if a confidential value (web Twitch/Google
secret, proxy token) appears — including the case where `.env.desktop` is
missing and the build silently falls back to web mode. It runs at the end of
`pnpm server:build` / `pnpm prepare:server`, or on demand:

```bash
pnpm check:secrets
```

See [SECURITY.md](./SECURITY.md) for the full threat model, why the Google
desktop secret is safe to embed, and how to isolate the OAuth client (dedicated
Google Cloud project, quotas and abuse alerts).

## Linux sandbox note

On distros that restrict unprivileged user namespaces (e.g. Ubuntu 24.04),
Electron aborts with a `SUID sandbox` error. Either run
`MIXCHAT_NO_SANDBOX=1 pnpm dev`, or fix the helper:

```bash
sudo chown root:root node_modules/.pnpm/electron@*/node_modules/electron/dist/chrome-sandbox
sudo chmod 4755 node_modules/.pnpm/electron@*/node_modules/electron/dist/chrome-sandbox
```

## Notes

- The app icon is generated from Mixchat's `public/favicon.png` (see
  `build/icon.png`). Unsigned builds make Windows SmartScreen warn — see
  [Code signing](#code-signing).

Licensed under AGPL-3.0 (see [LICENSE](./LICENSE)), same as Mixchat.

# Security

This document describes what ships inside Mixchat Desktop binaries, why that is
acceptable, and how to reduce risk when distributing. It complements the OAuth
section of the [README](./README.md).

## Threat model

The desktop app is a locally run Tauri shell around the Mixchat Astro server.
The end user owns the machine and can read every file the app ships, including
the server bundle and the Node sidecar. Therefore **any value baked into the
build must be assumed public**. Encryption or obfuscation only delays
extraction.

The app also bundles a Node runtime. Treat it like any other executable in the
installer. The server binds to `127.0.0.1` by default and the webview loads a
loopback origin.

## What is embedded

The build profile is chosen by whether `.env.desktop` exists at build time (see
`scripts/build-mixchat.mjs`).

| Value | Embedded? | Notes |
|---|---|---|
| Public Twitch client ID | Yes | Public identifier, not a secret |
| Twitch client secret | **No** | Desktop uses the Device Code Grant (public client) |
| Public Google/YouTube client ID | Yes | Public identifier |
| Google **Desktop-app** client secret | Yes | Non-confidential by Google's classification |
| Hosted-site Google web client ID | No | Overridden by `.env.desktop` |
| Hosted-site Google web client secret | **No** | Build guard fails if present |
| `YT_PROXY` / `YT_PROXY_TOKEN` | **No** | Never set by the desktop profile |
| Twitch GQL client ID | Yes | Public (Twitch's own web client id) |

## Why the Google desktop client secret is not a problem

Google documents that the client secret for **Desktop app** OAuth clients is
**not treated as a secret**. It is routinely embedded in installed apps such as
gcloud, rclone and the GitHub CLI. A leaked secret alone is useless because:

- Exchanging the secret for tokens still requires an authorization `code` bound
  to the registered redirect URI (`http://localhost:3000/...`), which can only
  be produced by a user completing the consent flow.
- The desktop flow uses **PKCE**. The `code_verifier` is generated per login and
  kept only in a short-lived pending handoff record. Without the matching
  verifier the code cannot be redeemed.

An attacker who extracts the secret cannot access any user's Twitch or YouTube
account.

### Residual risks

The realistic risks of exposing the desktop client ID and secret are:

- **Brand and consent impersonation**: someone can build an app that reuses your
  client ID so Google's consent screen shows Mixchat and your OAuth branding.
- **Quota and abuse**: your Google Cloud project's OAuth quota can be consumed,
  and abusive traffic can get the OAuth client or project suspended by Google.

## Recommended: isolate the desktop OAuth client

1. Use a dedicated Google Cloud project for the desktop Desktop-app OAuth
   client, separate from the website's project.
2. Configure the OAuth consent screen with the correct app name, logo and
   support links.
3. Set quotas and budgets, and enable budget and abuse alerts.
4. Monitor token issuance and error rates in Google Cloud logs.
5. Rotate the client secret if a release binary is suspected to have been
   abused. Ship an updated build, since existing installs keep the old value.

For Twitch, use a **Public** client so there is no secret to leak.

## Auth handoff

Google sign-in runs in the system browser, so the app window cannot receive the
session through `window.opener`. The server keeps a pending handoff record with
these properties:

- random 48-character state and handoff IDs
- PKCE verifier generated per login
- 10 minute TTL
- at most 20 pending records
- single use, consumed by the first successful poll
- results are only returned to the app that holds the matching handoff ID

The poll response sets the httpOnly auth cookies in the app's own cookie jar.
The system browser never receives them.

## Build guard

`scripts/check-secrets.mjs` fails the build if any confidential value reaches
the bundle. It runs at the end of `pnpm server:build` and `pnpm tauri:prepare`,
and can be run standalone:

```bash
pnpm check:secrets
```

It scans the compiled output for:

- the hosted site's sensitive values (`*_SECRET`, `*_TOKEN`, `*_COOKIE`,
  `*_POTOKEN` from the environment and `../Mixchat/.env`) that are not also
  provided by `.env.desktop`; and
- any Google client secret (`GOCSPX-...`) that is not the one from
  `.env.desktop`.

This also makes the forgot-`.env.desktop` footgun safe. Such a build bakes the
web secrets and the guard refuses to proceed.

## Runtime boundaries

- The main window loads `http://localhost:<port>`. It is not granted remote
  Tauri IPC permissions, so page JavaScript cannot invoke shell commands.
- Navigation away from localhost is cancelled and opened in the system browser.
  `window.open` targets are opened in the system browser as well.
- The server sidecar is started by Rust. The sidecar name and arguments are fixed
  at build time.
- `POST /api/desktop/open` only exists in desktop builds, accepts only `http(s)`
  URLs, and spawns the system browser. It never navigates the webview.
- The Node child runs in its own process group or job object so it is killed with
  the app.
- Portable builds keep `mixchat-node`, the server bundle, and all app data next
  to the executable. Extract the zip to a folder the user controls. Do not place
  it in a shared or world-writable location, since anyone who can write there
  could replace the sidecar.

## Updates and code signing

Auto-update is not configured in this build. Windows installers are manual
installs for now. When auto-update is added, sign the NSIS installer and the
update manifests before publishing.

## Reporting a vulnerability

Please report suspected vulnerabilities privately to
`noreply@mixchat.grepfs.xyz`. Do not open a public issue for security reports.

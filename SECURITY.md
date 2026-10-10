# Security

This document describes what ships inside Mixchat Desktop binaries, why that is
acceptable, and how to reduce risk when distributing. It complements the "OAuth
& secrets" section of the [README](./README.md).

## Threat model

The desktop app is a locally-run Electron shell around the Mixchat server. The
end user owns the machine and can read every file the app ships, including the
JavaScript bundle. Therefore **any value baked into the build must be assumed
public** — there is no such thing as a hidden secret in a distributed desktop
binary. Encryption or obfuscation only delays extraction.

## What is embedded

The build profile is chosen by whether `.env.desktop` exists at build time (see
`scripts/build-mixchat.mjs`).

| Value | Embedded? | Notes |
|---|---|---|
| Public Twitch client ID | Yes | Public identifier, not a secret |
| Twitch client secret | **No** | Desktop uses the Device Code Grant (public client) |
| Public Google/YouTube client ID | Yes | Public identifier |
| Google **Desktop-app** client secret | Yes | Non-confidential by Google's classification (see below) |
| Hosted-site Google web client ID | No | Overridden by `.env.desktop` |
| Hosted-site Google web client secret | **No** | Build guard fails if present |
| `YT_PROXY` / `YT_PROXY_TOKEN` | **No** | Never set by the desktop profile |
| Twitch GQL client ID | Yes | Public (Twitch's own web client id) |

## Why the Google desktop client secret is not a problem

Google documents that the client secret for **"Desktop app"** OAuth clients is
**not treated as a secret** — it is routinely embedded in installed apps such as
gcloud, rclone and the GitHub CLI. It is non-confidential because a leaked
secret alone is useless:

- Exchanging the secret for tokens still requires an authorization `code` that
  is bound to the registered redirect URI (`http://localhost:3000/...`) and can
  only be produced by a user completing the consent flow.
- The desktop flow additionally uses **PKCE**: the `code_verifier` is generated
  per login with `randomBytes(32)` and kept in an httpOnly cookie
  (`src/pages/api/auth/google/pkce.ts`, `src/pages/auth/youtube/callback.ts`).
  Without the matching verifier the code cannot be redeemed.

An attacker who extracts the secret therefore **cannot** access any user's
Twitch or YouTube account.

### Residual risks

The realistic risks of exposing the desktop client ID + secret are:

- **Brand / consent impersonation** — someone can build an app that reuses your
  client ID so Google's consent screen shows "Mixchat" and your OAuth branding.
- **Quota and abuse** — your Google Cloud project's OAuth quota can be consumed,
  and abusive traffic can get the OAuth client or project **suspended** by
  Google, breaking login for legitimate users.

## Recommended: isolate the desktop OAuth client

`SECURITY.md` is guidance; the console steps are manual. To keep abuse from
ever affecting the hosted website:

1. **Use a dedicated Google Cloud project** for the desktop "Desktop app" OAuth
   client, separate from the website's project. Compromise or suspension of one
   then cannot affect the other.
2. **Configure the OAuth consent screen** with the correct app name, logo and
   support links so impersonation is easier to spot and report.
3. **Set quotas and budgets** on the project (OAuth/token endpoint quotas, API
   quotas) and enable **budget/abuse alerts** to be notified of abnormal use.
4. **Monitor** token-issuance volume and error rates in Google Cloud logs.
5. **Rotate** the client secret if a release binary is suspected to have been
   abused. Note: rotating changes the secret, so existing installs keep working
   only until their tokens refresh; ship an updated build to be safe.

For Twitch, use a **Public** client (Client Type = Public) so there is no secret
to leak at all.

## Build guard

`scripts/check-secrets.mjs` fails the build if any confidential value reaches
the bundle. It runs automatically at the end of `pnpm server:build`
(`build-mixchat.mjs`) and `pnpm prepare:server` (`prepare-mixchat.mjs`), and can
be run standalone:

```bash
pnpm check:secrets
```

It scans the compiled output for:

- the hosted site's sensitive values (`*_SECRET`, `*_TOKEN`, `*_COOKIE`,
  `*_POTOKEN` from the environment and `../Mixchat/.env`) that are **not** also
  provided by `.env.desktop`; and
- any Google client secret (`GOCSPX-…`) that is not the one from
  `.env.desktop`.

This also makes the "forgot `.env.desktop`" footgun safe: such a build bakes the
web secrets and the guard refuses to proceed.

## Reporting a vulnerability

Please report suspected vulnerabilities privately to
`noreply@mixchat.grepfs.xyz`. Do not open a public issue for security reports.

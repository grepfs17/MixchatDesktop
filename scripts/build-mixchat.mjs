import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { parseEnvFile } from "../src/main/mixchat-server.mjs";
import { runSecretCheck } from "./check-secrets.mjs";

const projectRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
);
const mixchatRoot = process.env.MIXCHAT_DIR
  ? path.resolve(process.env.MIXCHAT_DIR)
  : path.resolve(projectRoot, "..", "Mixchat");

// Desktop build profile. When present, the Mixchat bundle is built in
// "desktop" mode: public-client OAuth (Twitch Device Code, Google PKCE) and
// NO client secrets baked in. See .env.desktop.example.
const desktopEnvFile = path.join(projectRoot, ".env.desktop");
const desktopMode = existsSync(desktopEnvFile);

const env = { ...process.env };
if (desktopMode) {
  const desktopEnv = parseEnvFile(desktopEnvFile);
  Object.assign(env, desktopEnv, { PUBLIC_APP_PLATFORM: "desktop" });
  // Twitch uses a Public app (Device Code Grant) -> never bake a Twitch secret.
  // Google does NOT support a secretless desktop flow (its token endpoint
  // rejects PKCE-only with "client_secret is missing"), so if `.env.desktop`
  // provides the Desktop client's secret we keep it; otherwise clear it so the
  // developer's web secret is never baked.
  // (TWITCH_GQL_CLIENT_ID is a public client id with a built-in fallback.)
  env.TWITCH_CLIENT_SECRET = "";
  if (!desktopEnv.YOUTUBE_CLIENT_SECRET) env.YOUTUBE_CLIENT_SECRET = "";
  console.log(
    "[build-mixchat] desktop mode: PUBLIC_APP_PLATFORM=desktop, Twitch secret stripped",
  );
} else {
  console.warn(
    "[build-mixchat] No .env.desktop found — building in web mode; client secrets WILL be baked.",
  );
}

const res = spawnSync("pnpm", ["--dir", mixchatRoot, "build"], {
  stdio: "inherit",
  env,
});
if ((res.status ?? 1) !== 0) process.exit(res.status ?? 1);

// Refuse to continue if confidential secrets (the hosted site's Twitch/Google
// secrets, the proxy token) ended up in the compiled bundle. A missing
// `.env.desktop` makes the build fall back to web mode and bake those secrets,
// so this guard also catches that footgun.
const { ok } = await runSecretCheck({
  distDir: path.join(mixchatRoot, "dist"),
  mixchatRoot,
  projectRoot,
});
process.exit(ok ? 0 : 1);

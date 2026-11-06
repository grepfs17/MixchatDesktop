import { readdir, readFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { parseEnvFile } from "./env.mjs";

/**
 * Build guard: fail the desktop build if any *confidential* secret (the hosted
 * website's Twitch / Google secrets, the YouTube proxy token, ...) ended up in
 * the compiled bundle.
 *
 * Desktop builds legitimately bake a Google "Desktop app" client secret, which
 * Google classes as non-confidential (see SECURITY.md). Every other sensitive
 * value must stay out. Sensitive values are discovered from the environment and
 * from `../Mixchat/.env`; a web value is allowed only if it is identical to a
 * value provided by `.env.desktop`.
 */

const SENSITIVE_SUFFIXES = ["_SECRET", "_TOKEN", "_COOKIE", "_POTOKEN"];
const MIN_SECRET_LEN = 8;
const GOOGLE_SECRET_RE = /GOCSPX-[A-Za-z0-9_-]{10,}/g;

/** A key is treated as sensitive unless it is explicitly public. */
export function isSensitiveKey(key) {
  if (!key || key.startsWith("PUBLIC_")) return false;
  return SENSITIVE_SUFFIXES.some((suffix) => key.endsWith(suffix));
}

/** Map of secretValue -> sourceKey for every sensitive entry in an env object. */
function sensitiveValues(env) {
  const out = new Map();
  for (const [key, value] of Object.entries(env ?? {})) {
    if (!isSensitiveKey(key)) continue;
    if (typeof value !== "string" || value.length < MIN_SECRET_LEN) continue;
    if (!out.has(value)) out.set(value, key);
  }
  return out;
}

async function* walkFiles(dir) {
  let entries;
  try {
    entries = await readdir(dir, { withFileTypes: true });
  } catch {
    return;
  }
  for (const entry of entries) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) yield* walkFiles(full);
    else if (entry.isFile()) yield full;
  }
}

/**
 * Scan `distDir` for forbidden secret values and stray Google secrets.
 * Returns { ok, hits, strayGoogleSecrets, scanned }.
 */
export async function scanForSecrets(distDir, forbidden, allowedGoogleSecrets) {
  const hits = [];
  const strayGoogleSecrets = new Set();
  let scanned = 0;

  for await (const file of walkFiles(distDir)) {
    let text;
    try {
      text = await readFile(file, "utf8");
    } catch {
      continue;
    }
    scanned += 1;

    for (const [value, key] of forbidden) {
      if (text.includes(value)) hits.push({ file, key });
    }

    const matches = text.match(GOOGLE_SECRET_RE);
    if (matches) {
      for (const match of matches) {
        if (!allowedGoogleSecrets.has(match)) strayGoogleSecrets.add(match);
      }
    }
  }

  return {
    ok: hits.length === 0 && strayGoogleSecrets.size === 0,
    hits,
    strayGoogleSecrets: [...strayGoogleSecrets],
    scanned,
  };
}

function mask(value) {
  if (value.length <= 8) return "****";
  return `${value.slice(0, 4)}…${value.slice(-2)}`;
}

/**
 * Run the guard for a build. Resolves env sources itself so callers only pass
 * the dist directory and roots.
 *
 * @returns {Promise<{ok: boolean, scanned: number}>}
 */
export async function runSecretCheck({
  distDir,
  mixchatRoot,
  projectRoot,
  allowGoogleSecrets = [],
  logger = console,
}) {
  const webEnvFile =
    process.env.MIXCHAT_WEB_ENV_FILE || path.join(mixchatRoot, ".env");
  const desktopEnvFile = path.join(projectRoot, ".env.desktop");

  const webEnv = existsSync(webEnvFile) ? parseEnvFile(webEnvFile) : {};
  const desktopEnv = existsSync(desktopEnvFile)
    ? parseEnvFile(desktopEnvFile)
    : {};

  // A value is allowed if the desktop profile also provides it (e.g. the
  // Desktop-app Google client secret).
  const allowedValues = new Set([
    ...sensitiveValues(desktopEnv).keys(),
    ...allowGoogleSecrets,
  ]);

  const candidates = new Map([
    ...sensitiveValues(process.env),
    ...sensitiveValues(webEnv),
  ]);
  const forbidden = new Map();
  for (const [value, key] of candidates) {
    if (!allowedValues.has(value)) forbidden.set(value, key);
  }

  const allowedGoogleSecrets = new Set(allowGoogleSecrets);
  for (const value of sensitiveValues(desktopEnv).keys()) {
    if (value.startsWith("GOCSPX-")) allowedGoogleSecrets.add(value);
  }

  if (!existsSync(distDir)) {
    logger.warn(`[check-secrets] dist not found, skipping: ${distDir}`);
    return { ok: true, scanned: 0 };
  }

  const { ok, hits, strayGoogleSecrets, scanned } = await scanForSecrets(
    distDir,
    forbidden,
    allowedGoogleSecrets,
  );

  if (ok) {
    logger.log(
      `[check-secrets] OK — ${scanned} file(s) scanned, no confidential secrets found.`,
    );
    return { ok, scanned };
  }

  logger.error("[check-secrets] FAILED: confidential secrets found in the bundle.");
  for (const { file, key } of hits) {
    logger.error(`  - ${key} leaked into ${path.relative(projectRoot, file)}`);
  }
  for (const value of strayGoogleSecrets) {
    logger.error(
      `  - unknown Google client secret (${mask(value)}) present but not from .env.desktop`,
    );
  }
  logger.error(
    "[check-secrets] Refusing to ship. Build with a proper .env.desktop profile, or remove the secret from the bundle.",
  );
  return { ok, scanned };
}

async function main() {
  const projectRoot = path.resolve(
    path.dirname(fileURLToPath(import.meta.url)),
    "..",
  );
  const mixchatRoot = process.env.MIXCHAT_DIR
    ? path.resolve(process.env.MIXCHAT_DIR)
    : path.resolve(projectRoot, "..", "Mixchat");

  const argDir = process.argv[2];
  const preparedDist = path.join(projectRoot, "build", "mixchat", "dist");
  const sourceDist = path.join(mixchatRoot, "dist");
  const distDir = argDir
    ? path.resolve(argDir)
    : existsSync(preparedDist)
      ? preparedDist
      : sourceDist;

  const { ok } = await runSecretCheck({ distDir, mixchatRoot, projectRoot });
  process.exit(ok ? 0 : 1);
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  main();
}

import { existsSync, readFileSync } from "node:fs";

/**
 * Minimal .env parser: supports comments, `export KEY=`, quoted values and
 * trailing whitespace. Not a full dotenv implementation, just enough to feed
 * Mixchat's build profile and runtime secrets into the spawned server.
 */
export function parseEnvFile(file) {
  const out = {};
  if (!file || !existsSync(file)) return out;
  const text = readFileSync(file, "utf8");
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    if (!line || line.startsWith("#")) continue;
    const normalized = line.startsWith("export ") ? line.slice(7).trim() : line;
    const eq = normalized.indexOf("=");
    if (eq === -1) continue;
    const key = normalized.slice(0, eq).trim();
    let value = normalized.slice(eq + 1).trim();
    if (
      (value.startsWith('"') && value.endsWith('"')) ||
      (value.startsWith("'") && value.endsWith("'"))
    ) {
      value = value.slice(1, -1);
    }
    if (key) out[key] = value;
  }
  return out;
}

/** Pick the first existing .env file from a list of candidates. */
export function findEnvFile(candidates) {
  for (const candidate of candidates) {
    if (candidate && existsSync(candidate)) return candidate;
  }
  return null;
}

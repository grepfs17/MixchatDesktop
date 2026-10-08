import { spawn } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import path from "node:path";

/**
 * Minimal .env parser: supports comments, `export KEY=`, quoted values and
 * trailing whitespace. Not a full dotenv implementation, just enough to feed
 * Mixchat's runtime secrets into the spawned server process.
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

function prefixLines(stream, prefix, onLine) {
  let buffer = "";
  stream.setEncoding("utf8");
  stream.on("data", (chunk) => {
    buffer += chunk;
    const lines = buffer.split(/\r?\n/);
    buffer = lines.pop() ?? "";
    for (const line of lines) onLine(prefix + line);
  });
  stream.on("end", () => {
    if (buffer) onLine(prefix + buffer);
  });
}

/**
 * Spawn Mixchat's Astro Node standalone server using Electron's bundled Node
 * (ELECTRON_RUN_AS_NODE), so no system Node install is required when packaged.
 */
export function startMixchatServer({ root, entry, env, cwd, onLog }) {
  if (!existsSync(entry)) {
    throw new Error(
      `Mixchat server build not found at ${entry}. Run "pnpm build" inside ${root} first.`,
    );
  }

  const child = spawn(process.execPath, [entry], {
    cwd: cwd || root,
    env: { ...env, ELECTRON_RUN_AS_NODE: "1" },
    stdio: ["ignore", "pipe", "pipe"],
  });

  if (onLog) {
    prefixLines(child.stdout, "[mixchat] ", onLog);
    prefixLines(child.stderr, "[mixchat:err] ", onLog);
  }

  return child;
}

/** Poll the server until it answers or the timeout elapses. */
export async function waitForServer(url, timeoutMs = 60000, child = null) {
  const deadline = Date.now() + timeoutMs;
  let lastError = null;
  while (Date.now() < deadline) {
    if (child && child.exitCode !== null) {
      throw new Error(
        `Mixchat server exited early with code ${child.exitCode}. Check the logs above.`,
      );
    }
    try {
      const res = await fetch(url, { method: "GET", redirect: "manual" });
      if (res.status > 0) return true;
    } catch (error) {
      lastError = error;
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error(`Timed out waiting for Mixchat at ${url}: ${lastError ?? "no response"}`);
}

export const mixchatServerEntry = (root) =>
  path.join(root, "dist", "server", "entry.mjs");

import { cp, readFile, writeFile, rm } from "node:fs/promises";
import { existsSync } from "node:fs";
import { pathToFileURL } from "node:url";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { runSecretCheck } from "./check-secrets.mjs";

const projectRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
);
const mixchatRoot = process.env.MIXCHAT_DIR
  ? path.resolve(process.env.MIXCHAT_DIR)
  : path.resolve(projectRoot, "..", "Mixchat");
const srcDist = path.join(mixchatRoot, "dist");
const outRoot = path.join(projectRoot, "build", "mixchat");
const outDist = path.join(outRoot, "dist");

if (!existsSync(path.join(srcDist, "server", "entry.mjs"))) {
  console.error(
    `[prepare] No Mixchat build found at ${srcDist}. Run "pnpm --dir ${mixchatRoot} build" first.`,
  );
  process.exit(1);
}

// Astro's Node adapter bakes the build machine's absolute paths into the server
// bundle (client/server dirs, session storage base). Those break on another OS
// (e.g. file:///home/... is invalid on Windows). OpenSSL note: the client/server
// URLs are only used to compute a relative offset, so any valid absolute URL
// with the same client/ + server/ sibling layout works.
const rootUrl = pathToFileURL(mixchatRoot).href.replace(/\/?$/, "/");
const sessionBase = path.join(mixchatRoot, "node_modules", ".astro", "sessions");
// Must be a drive-absolute file URL for Windows `fileURLToPath` to accept it.
// The value is never read from disk — it only anchors the relative client/
// server offset — so a synthetic C:/ path is safe on every platform.
const PORTABLE_ROOT_URL = "file:///C:/mixchat/";
const PORTABLE_SESSION_BASE = "./mixchat-sessions";

console.log(`[prepare] source: ${srcDist}`);
console.log(`[prepare] output: ${outDist}`);

await rm(outRoot, { recursive: true, force: true });
await cp(srcDist, outDist, { recursive: true });

const entryPath = path.join(outDist, "server", "entry.mjs");
let entry = await readFile(entryPath, "utf8");

let urlReplacements = 0;
if (entry.includes(rootUrl)) {
  urlReplacements = entry.split(rootUrl).length - 1;
  entry = entry.split(rootUrl).join(PORTABLE_ROOT_URL);
}
let sessionReplacements = 0;
if (entry.includes(sessionBase)) {
  sessionReplacements = entry.split(sessionBase).length - 1;
  entry = entry.split(sessionBase).join(PORTABLE_SESSION_BASE);
}

await writeFile(entryPath, entry);

console.log(
  `[prepare] patched ${urlReplacements} build-root URL(s), ${sessionReplacements} session path(s).`,
);
if (urlReplacements === 0 && sessionReplacements === 0) {
  console.warn(
    "[prepare] WARNING: no baked absolute paths were found. The bundle may already be portable, or the expected build layout changed.",
  );
}

const { ok: secretsOk } = await runSecretCheck({
  distDir: outDist,
  mixchatRoot,
  projectRoot,
});
if (!secretsOk) process.exit(1);

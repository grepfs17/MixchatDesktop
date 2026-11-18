import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";

import {
  isSensitiveKey,
  isSensitiveValue,
  scanForSecrets,
  runSecretCheck,
} from "./check-secrets.mjs";

const silentLogger = { log() {}, warn() {}, error() {} };

async function makeTemp(prefix) {
  const dir = await mkdtemp(path.join(os.tmpdir(), prefix));
  return dir;
}

async function writeFileAt(root, relative, contents) {
  const file = path.join(root, relative);
  await mkdir(path.dirname(file), { recursive: true });
  await writeFile(file, contents);
  return file;
}

function isolateWebEnvOverride(t) {
  const previous = process.env.MIXCHAT_WEB_ENV_FILE;
  delete process.env.MIXCHAT_WEB_ENV_FILE;
  t.after(() => {
    if (previous === undefined) delete process.env.MIXCHAT_WEB_ENV_FILE;
    else process.env.MIXCHAT_WEB_ENV_FILE = previous;
  });
}

test("isSensitiveKey ignores public keys and flags secret suffixes", () => {
  assert.equal(isSensitiveKey("PUBLIC_TWITCH_CLIENT_ID"), false);
  assert.equal(isSensitiveKey("TWITCH_CLIENT_SECRET"), true);
  assert.equal(isSensitiveKey("YOUTUBE_REFRESH_TOKEN"), true);
  assert.equal(isSensitiveKey("SESSION_COOKIE"), true);
  assert.equal(isSensitiveKey("PLAIN_VALUE"), false);
  assert.equal(isSensitiveKey(""), false);
});

test("isSensitiveValue detects credentials embedded in a URL", () => {
  assert.equal(isSensitiveValue("postgres://user:pass@db.test/app"), true);
  assert.equal(isSensitiveValue("https://user:p@host.test"), true);
  assert.equal(isSensitiveValue("https://host.test/plain"), false);
  assert.equal(isSensitiveValue("not-a-url"), false);
  assert.equal(isSensitiveValue(undefined), false);
});

test("scanForSecrets reports forbidden values", async (t) => {
  const dist = await makeTemp("mixchat-dist-");
  t.after(() => rm(dist, { recursive: true, force: true }));
  await writeFileAt(dist, "assets/app.js", "const token = 'super-secret-value';");

  const result = await scanForSecrets(
    dist,
    new Map([["super-secret-value", "TWITCH_CLIENT_SECRET"]]),
    new Set(),
  );

  assert.equal(result.ok, false);
  assert.equal(result.scanned, 1);
  assert.equal(result.hits.length, 1);
  assert.match(result.hits[0].file, /assets\/app\.js$/);
  assert.equal(result.hits[0].key, "TWITCH_CLIENT_SECRET");
});

test("scanForSecrets passes a clean bundle", async (t) => {
  const dist = await makeTemp("mixchat-dist-");
  t.after(() => rm(dist, { recursive: true, force: true }));
  await writeFileAt(dist, "index.html", "<html>Mixchat</html>");

  const result = await scanForSecrets(dist, new Map(), new Set());

  assert.equal(result.ok, true);
  assert.equal(result.scanned, 1);
  assert.deepEqual(result.hits, []);
  assert.deepEqual(result.strayGoogleSecrets, []);
});

test("scanForSecrets flags unknown Google secrets but allows listed ones", async (t) => {
  const dist = await makeTemp("mixchat-dist-");
  t.after(() => rm(dist, { recursive: true, force: true }));
  const allowed = "GOCSPX-allowedSecret123";
  const stray = "GOCSPX-straySecret4567";
  await writeFileAt(dist, "bundle.js", `${allowed} ${stray}`);

  const result = await scanForSecrets(dist, new Map(), new Set([allowed]));

  assert.equal(result.ok, false);
  assert.deepEqual(result.strayGoogleSecrets, [stray]);
});

test("runSecretCheck fails when the dist is missing", async (t) => {
  isolateWebEnvOverride(t);
  const root = await makeTemp("mixchat-root-");
  t.after(() => rm(root, { recursive: true, force: true }));

  const result = await runSecretCheck({
    distDir: path.join(root, "missing-dist"),
    mixchatRoot: root,
    projectRoot: root,
    logger: silentLogger,
  });

  assert.equal(result.ok, false);
  assert.equal(result.scanned, 0);
});

test("runSecretCheck can skip a missing dist only when allowed", async (t) => {
  isolateWebEnvOverride(t);
  const root = await makeTemp("mixchat-root-");
  t.after(() => rm(root, { recursive: true, force: true }));

  const result = await runSecretCheck({
    distDir: path.join(root, "missing-dist"),
    mixchatRoot: root,
    projectRoot: root,
    allowMissingDist: true,
    logger: silentLogger,
  });

  assert.equal(result.ok, true);
  assert.equal(result.scanned, 0);
});

test("runSecretCheck blocks a web secret found in the bundle", async (t) => {
  isolateWebEnvOverride(t);
  const root = await makeTemp("mixchat-root-");
  t.after(() => rm(root, { recursive: true, force: true }));
  const mixchatRoot = path.join(root, "mixchat");
  const projectRoot = path.join(root, "app");
  await writeFileAt(mixchatRoot, ".env", "TWITCH_CLIENT_SECRET=leaked-value-123\n");
  await writeFileAt(projectRoot, "dist/index.html", "leaked-value-123");

  const result = await runSecretCheck({
    distDir: path.join(projectRoot, "dist"),
    mixchatRoot,
    projectRoot,
    logger: silentLogger,
  });

  assert.equal(result.ok, false);
});

test("runSecretCheck allows a secret also provided by .env.desktop", async (t) => {
  isolateWebEnvOverride(t);
  const root = await makeTemp("mixchat-root-");
  t.after(() => rm(root, { recursive: true, force: true }));
  const mixchatRoot = path.join(root, "mixchat");
  const projectRoot = path.join(root, "app");
  await writeFileAt(
    mixchatRoot,
    ".env",
    "YOUTUBE_CLIENT_SECRET=shared-desktop-secret\n",
  );
  await writeFileAt(
    projectRoot,
    ".env.desktop",
    "YOUTUBE_CLIENT_SECRET=shared-desktop-secret\n",
  );
  await writeFileAt(projectRoot, "dist/index.html", "shared-desktop-secret");

  const result = await runSecretCheck({
    distDir: path.join(projectRoot, "dist"),
    mixchatRoot,
    projectRoot,
    logger: silentLogger,
  });

  assert.equal(result.ok, true);
});

test("runSecretCheck allows the desktop Google secret and flags unknown ones", async (t) => {
  isolateWebEnvOverride(t);
  const root = await makeTemp("mixchat-root-");
  t.after(() => rm(root, { recursive: true, force: true }));
  const mixchatRoot = path.join(root, "mixchat");
  const projectRoot = path.join(root, "app");
  const desktopSecret = "GOCSPX-desktopSecret999";
  await writeFileAt(
    mixchatRoot,
    ".env",
    `GOOGLE_CLIENT_SECRET=${desktopSecret}\n`,
  );
  await writeFileAt(
    projectRoot,
    ".env.desktop",
    `YOUTUBE_CLIENT_SECRET=${desktopSecret}\n`,
  );

  await writeFileAt(projectRoot, "dist/ok.html", desktopSecret);
  const allowed = await runSecretCheck({
    distDir: path.join(projectRoot, "dist"),
    mixchatRoot,
    projectRoot,
    logger: silentLogger,
  });
  assert.equal(allowed.ok, true);

  await writeFileAt(projectRoot, "dist/leak.html", "GOCSPX-unknownSecret000");
  const leaked = await runSecretCheck({
    distDir: path.join(projectRoot, "dist"),
    mixchatRoot,
    projectRoot,
    logger: silentLogger,
  });
  assert.equal(leaked.ok, false);
});

test("runSecretCheck does not flag PUBLIC_ values", async (t) => {
  isolateWebEnvOverride(t);
  const root = await makeTemp("mixchat-root-");
  t.after(() => rm(root, { recursive: true, force: true }));
  const mixchatRoot = path.join(root, "mixchat");
  const projectRoot = path.join(root, "app");
  await writeFileAt(
    mixchatRoot,
    ".env",
    "PUBLIC_TWITCH_CLIENT_ID=public-client-value\n",
  );
  await writeFileAt(projectRoot, "dist/index.html", "public-client-value");

  const result = await runSecretCheck({
    distDir: path.join(projectRoot, "dist"),
    mixchatRoot,
    projectRoot,
    logger: silentLogger,
  });

  assert.equal(result.ok, true);
});

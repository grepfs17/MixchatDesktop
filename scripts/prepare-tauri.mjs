import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { cp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { parseEnvFile } from "./env.mjs";
import { runSecretCheck } from "./check-secrets.mjs";

const projectRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
);
const mixchatRoot = process.env.MIXCHAT_DIR
  ? path.resolve(process.env.MIXCHAT_DIR)
  : path.resolve(projectRoot, "..", "Mixchat");
const source = path.join(projectRoot, "build", "mixchat");
const destination = path.join(projectRoot, "src-tauri", "resources", "mixchat");
const desktopEnvFile = path.join(projectRoot, ".env.desktop");
const desktopEnv = existsSync(desktopEnvFile)
  ? parseEnvFile(desktopEnvFile)
  : {};

if (!desktopEnv.PUBLIC_TWITCH_CLIENT_ID) {
  console.warn(
    "[tauri-prepare] PUBLIC_TWITCH_CLIENT_ID is empty. Twitch sign-in will fail.",
  );
}
if (!desktopEnv.PUBLIC_YOUTUBE_CLIENT_ID) {
  console.warn(
    "[tauri-prepare] PUBLIC_YOUTUBE_CLIENT_ID is empty. YouTube sign-in will fail.",
  );
}

if (!existsSync(path.join(source, "dist", "server", "entry.mjs"))) {
  console.error(
    `[tauri-prepare] No prepared Mixchat bundle at ${source}. Run "pnpm prepare:server" first.`,
  );
  process.exit(1);
}

await mkdir(destination, { recursive: true });
await rm(path.join(destination, "dist"), { recursive: true, force: true });
await cp(path.join(source, "dist"), path.join(destination, "dist"), {
  recursive: true,
});
console.log(
  `[tauri-prepare] copied ${path.join(source, "dist")} -> ${path.join(destination, "dist")}`,
);

await ensureJsdom();

// Re-scan the *shipped* copy: the guard earlier runs on build/mixchat/dist,
// but what actually lands in the binary is src-tauri/resources/mixchat.
{
  const { ok } = await runSecretCheck({
    distDir: path.join(destination, "dist"),
    mixchatRoot,
    projectRoot,
  });
  if (!ok) process.exit(1);
}

/**
 * jsdom is the only runtime package kept external to the server bundle. It
 * reads a stylesheet relative to __dirname, which does not survive ESM
 * bundling. Install it next to the bundle so Node resolves it from
 * resources/mixchat/node_modules.
 *
 * The install is pinned by a vendored lockfile (scripts/jsdom-lock) so the
 * shipped transitive closure is reproducible; `--frozen-lockfile` refuses to
 * resolve fresh versions at build time.
 */
async function ensureJsdom() {
  const sourcePackage = path.join(
    mixchatRoot,
    "node_modules",
    "jsdom",
    "package.json",
  );
  if (!existsSync(sourcePackage)) {
    throw new Error(
      `[tauri-prepare] jsdom is not installed in ${mixchatRoot}. Run "pnpm install" there first.`,
    );
  }
  const version = JSON.parse(await readFile(sourcePackage, "utf8")).version;

  const installedPackage = path.join(
    destination,
    "node_modules",
    "jsdom",
    "package.json",
  );
  if (existsSync(installedPackage)) {
    const installed = JSON.parse(await readFile(installedPackage, "utf8"))
      .version;
    if (installed === version) {
      console.log(`[tauri-prepare] jsdom ${version} already present`);
      return;
    }
  }

  const lockDir = path.join(projectRoot, "scripts", "jsdom-lock");
  const lockPackage = path.join(lockDir, "package.json");
  const lockFile = path.join(lockDir, "pnpm-lock.yaml");
  if (!existsSync(lockPackage) || !existsSync(lockFile)) {
    throw new Error(
      `[tauri-prepare] missing vendored jsdom lockfile in ${lockDir}. ` +
        `Regenerate with: cd scripts/jsdom-lock && pnpm install --prod --node-linker=hoisted --ignore-scripts`,
    );
  }
  const lockedVersion = JSON.parse(await readFile(lockPackage, "utf8"))
    .dependencies?.jsdom;
  if (lockedVersion !== version) {
    throw new Error(
      `[tauri-prepare] Mixchat uses jsdom ${version} but scripts/jsdom-lock pins ${lockedVersion}. ` +
        `Update the lockfile: cd scripts/jsdom-lock && ` +
        `printf '{"private":true,"dependencies":{"jsdom":"%s"}}\\n' '${version}' > package.json && ` +
        `rm -f pnpm-lock.yaml && pnpm install --prod --node-linker=hoisted --ignore-scripts`,
    );
  }

  await writeFile(
    path.join(destination, "package.json"),
    `${JSON.stringify(
      { private: true, dependencies: { jsdom: version } },
      null,
      2,
    )}\n`,
    "utf8",
  );
  await cp(lockFile, path.join(destination, "pnpm-lock.yaml"));

  const pnpm = process.platform === "win32" ? "pnpm.cmd" : "pnpm";
  const result = spawnSync(
    pnpm,
    [
      "install",
      "--prod",
      "--node-linker=hoisted",
      "--ignore-scripts",
      "--frozen-lockfile",
    ],
    { cwd: destination, stdio: "inherit" },
  );
  if ((result.status ?? 1) !== 0) {
    throw new Error(
      "[tauri-prepare] failed to install jsdom for the server sidecar",
    );
  }
  console.log(`[tauri-prepare] installed jsdom ${version} (frozen lockfile)`);
}

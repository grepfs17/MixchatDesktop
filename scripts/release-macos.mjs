import { existsSync, readdirSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import { runPnpm } from "./pnpm.mjs";

const projectRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
);
const tauriCli = path.join(
  projectRoot,
  "node_modules",
  "@tauri-apps",
  "cli",
  "tauri.js",
);

// `pnpm tauri:prepare` runs once below; the inline config override nulls out
// beforeBuildCommand so `tauri build` does not rebuild the Mixchat bundle again.
const skipPrepare = JSON.stringify({ build: { beforeBuildCommand: null } });

function run(command, args) {
  console.log(`[release-macos] $ ${path.basename(command)} ${args.join(" ")}`);
  const result = spawnSync(command, args, {
    cwd: projectRoot,
    stdio: "inherit",
  });
  if (result.error) {
    console.error(
      `[release-macos] failed to start: ${result.error.message}`,
    );
  }
  if ((result.status ?? 1) !== 0) process.exit(result.status ?? 1);
}

if (process.platform !== "darwin") {
  console.error(
    "[release-macos] This script builds the macOS app and DMG on a macOS host.",
  );
  process.exit(1);
}

console.log("[release-macos] $ pnpm tauri:prepare");
const prepare = runPnpm(["tauri:prepare"], { cwd: projectRoot });
if ((prepare.status ?? 1) !== 0) process.exit(prepare.status ?? 1);

run(process.execPath, [
  tauriCli,
  "build",
  "--bundles",
  "app,dmg",
  "--config",
  skipPrepare,
]);

const dmgDir = path.join(
  projectRoot,
  "src-tauri",
  "target",
  "release",
  "bundle",
  "dmg",
);
const macosDir = path.join(
  projectRoot,
  "src-tauri",
  "target",
  "release",
  "bundle",
  "macos",
);
const images = existsSync(dmgDir)
  ? readdirSync(dmgDir).filter((name) => name.endsWith(".dmg"))
  : [];
const updaterBundles = existsSync(macosDir)
  ? readdirSync(macosDir).filter((name) => name.endsWith(".app.tar.gz"))
  : [];

if (images.length === 0) {
  console.error("[release-macos] DMG missing from target/release/bundle/dmg/");
  process.exit(1);
}
if (updaterBundles.length === 0) {
  console.error(
    "[release-macos] updater bundle missing from target/release/bundle/macos/",
  );
  process.exit(1);
}
const signature = `${updaterBundles[0]}.sig`;
if (!existsSync(path.join(macosDir, signature))) {
  console.error(
    "[release-macos] updater signature missing. Set TAURI_SIGNING_PRIVATE_KEY before building.",
  );
  process.exit(1);
}
console.log(
  `[release-macos] dmg: src-tauri/target/release/bundle/dmg/${images[0]}`,
);
console.log(
  `[release-macos] updater: src-tauri/target/release/bundle/macos/${updaterBundles[0]}`,
);
console.log(
  `[release-macos] signature: src-tauri/target/release/bundle/macos/${signature}`,
);

import { spawnSync } from "node:child_process";
import { existsSync, readdirSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
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
  console.log(`[release-linux] $ ${path.basename(command)} ${args.join(" ")}`);
  const result = spawnSync(command, args, {
    cwd: projectRoot,
    stdio: "inherit",
  });
  if (result.error) {
    console.error(
      `[release-linux] failed to start: ${result.error.message}`,
    );
  }
  if ((result.status ?? 1) !== 0) process.exit(result.status ?? 1);
}

if (process.platform === "win32") {
  console.error(
    "[release-linux] This script builds the Linux AppImage. On Windows run: pnpm release",
  );
  process.exit(1);
}

console.log("[release-linux] $ pnpm tauri:prepare");
const prepare = runPnpm(["tauri:prepare"], { cwd: projectRoot });
if ((prepare.status ?? 1) !== 0) process.exit(prepare.status ?? 1);

run(process.execPath, [
  tauriCli,
  "build",
  "--bundles",
  "appimage",
  "--config",
  skipPrepare,
]);

const appImageDir = path.join(
  projectRoot,
  "src-tauri",
  "target",
  "release",
  "bundle",
  "appimage",
);
const images = existsSync(appImageDir)
  ? readdirSync(appImageDir).filter((name) => name.endsWith(".AppImage"))
  : [];

if (images.length === 0) {
  console.error(
    "[release-linux] AppImage missing from target/release/bundle/appimage/",
  );
  process.exit(1);
}
console.log(
  `[release-linux] appimage: src-tauri/target/release/bundle/appimage/${images[0]}`,
);

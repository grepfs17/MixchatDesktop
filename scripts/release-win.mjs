import { spawnSync } from "node:child_process";
import { existsSync, readdirSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const projectRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
);
const pnpm = process.platform === "win32" ? "pnpm.cmd" : "pnpm";

// `pnpm tauri:prepare` runs once below; the inline config override nulls out
// beforeBuildCommand so each `tauri build` does not rebuild the Mixchat
// bundle a second and third time.
const skipPrepare = JSON.stringify({ build: { beforeBuildCommand: null } });

function run(command, args) {
  console.log(`[release] $ ${command} ${args.join(" ")}`);
  const result = spawnSync(command, args, {
    cwd: projectRoot,
    stdio: "inherit",
  });
  if ((result.status ?? 1) !== 0) process.exit(result.status ?? 1);
}

run(pnpm, ["tauri:prepare"]);

run(pnpm, ["exec", "tauri", "build", "--config", skipPrepare]);

run(pnpm, [
  "exec",
  "tauri",
  "build",
  "--no-bundle",
  "--config",
  "src-tauri/tauri.portable.conf.json",
  "--config",
  skipPrepare,
]);

run("node", ["scripts/make-portable.mjs"]);

// The tauri CLI only bundles `nsis` on a Windows host — on other platforms the
// target is filtered out silently. Fail loudly on Windows if the installer is
// missing; explain the skip elsewhere (CI builds it on windows-latest).
const nsisDir = path.join(
  projectRoot,
  "src-tauri",
  "target",
  "release",
  "bundle",
  "nsis",
);
const installers = existsSync(nsisDir)
  ? readdirSync(nsisDir).filter((name) => name.endsWith("-setup.exe"))
  : [];

if (process.platform === "win32") {
  if (installers.length === 0) {
    console.error("[release] NSIS installer missing from target/release/bundle/nsis/");
    process.exit(1);
  }
  console.log(
    `[release] installer: src-tauri/target/release/bundle/nsis/${installers[0]}`,
  );
} else {
  console.warn(
    "[release] NSIS installer skipped — the tauri CLI bundles nsis only on a " +
      "Windows host (CI builds it on windows-latest).",
  );
}
console.log("[release] portable:  src-tauri/target/portable/");

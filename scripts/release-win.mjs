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
// beforeBuildCommand so each `tauri build` does not rebuild the Mixchat
// bundle a second and third time.
const skipPrepare = JSON.stringify({ build: { beforeBuildCommand: null } });

function run(command, args) {
  console.log(`[release] $ ${path.basename(command)} ${args.join(" ")}`);
  const result = spawnSync(command, args, {
    cwd: projectRoot,
    stdio: "inherit",
  });
  if (result.error) {
    console.error(`[release] failed to start: ${result.error.message}`);
  }
  if ((result.status ?? 1) !== 0) process.exit(result.status ?? 1);
}

console.log("[release] $ pnpm tauri:prepare");
const prepare = runPnpm(["tauri:prepare"], { cwd: projectRoot });
if ((prepare.status ?? 1) !== 0) process.exit(prepare.status ?? 1);

// The Tauri CLI is invoked through its JS entry directly: `pnpm exec` would
// need a shell on Windows, and cmd.exe would mangle the quoted JSON config.
run(process.execPath, [tauriCli, "build", "--config", skipPrepare]);

run(process.execPath, [
  tauriCli,
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
  const signature = `${installers[0]}.sig`;
  if (!existsSync(path.join(nsisDir, signature))) {
    console.error(
      "[release] updater signature missing. Set TAURI_SIGNING_PRIVATE_KEY before building.",
    );
    process.exit(1);
  }
  console.log(
    `[release] installer: src-tauri/target/release/bundle/nsis/${installers[0]}`,
  );
  console.log(
    `[release] signature: src-tauri/target/release/bundle/nsis/${signature}`,
  );

  const portableDir = path.join(projectRoot, "src-tauri", "target", "portable");
  const portableZips = existsSync(portableDir)
    ? readdirSync(portableDir).filter((name) =>
        name.endsWith("-portable.zip"),
      )
    : [];
  if (portableZips.length === 0) {
    console.error("[release] portable zip missing from target/portable/");
    process.exit(1);
  }
  const portableSignature = `${portableZips[0]}.sig`;
  if (!existsSync(path.join(portableDir, portableSignature))) {
    console.error(
      "[release] portable updater signature missing. Set TAURI_SIGNING_PRIVATE_KEY before building.",
    );
    process.exit(1);
  }
  console.log(
    `[release] portable signature: src-tauri/target/portable/${portableSignature}`,
  );
} else {
  console.warn(
    "[release] NSIS installer skipped — the tauri CLI bundles nsis only on a " +
      "Windows host (CI builds it on windows-latest).",
  );
}
console.log("[release] portable:  src-tauri/target/portable/");

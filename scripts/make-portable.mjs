import { execFileSync } from "node:child_process";
import { existsSync } from "node:fs";
import {
  cp,
  mkdir,
  readFile,
  rm,
  stat,
  writeFile,
} from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const projectRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
);
const tauriRoot = path.join(projectRoot, "src-tauri");
const portableRoot = path.join(tauriRoot, "target", "portable");
const folder = path.join(portableRoot, "Mixchat");
const args = process.argv.slice(2);
const targetIndex = args.indexOf("--target");
const targetTriple = targetIndex >= 0 ? args[targetIndex + 1] : null;

if (targetIndex >= 0 && !targetTriple) {
  console.error("[portable] --target requires a Rust target triple");
  process.exit(1);
}

function platformName(triple) {
  if (!triple) {
    return `${process.platform}-${process.arch}`.replace("win32", "win");
  }
  if (triple.includes("windows")) {
    return triple.startsWith("aarch64") ? "win-arm64" : "win-x64";
  }
  if (triple.includes("linux")) {
    return triple.startsWith("aarch64") ? "linux-arm64" : "linux-x64";
  }
  if (triple.includes("darwin")) {
    return triple.startsWith("aarch64") ? "macos-arm64" : "macos-x64";
  }
  return triple;
}

const isWindows = targetTriple
  ? targetTriple.includes("windows")
  : process.platform === "win32";
const releaseDir = targetTriple
  ? path.join(tauriRoot, "target", targetTriple, "release")
  : path.join(tauriRoot, "target", "release");
const exeName = isWindows ? "mixchat-desktop.exe" : "mixchat-desktop";
const sidecarName = isWindows ? "mixchat-node.exe" : "mixchat-node";
const appName = isWindows ? "Mixchat.exe" : "Mixchat";
const platform = platformName(targetTriple);
const packageJson = JSON.parse(
  await readFile(path.join(projectRoot, "package.json"), "utf8"),
);
const zipName = `Mixchat-${packageJson.version}-${platform}-portable.zip`;
const zipPath = path.join(portableRoot, zipName);

for (const required of [exeName, sidecarName, "mixchat"]) {
  if (!existsSync(path.join(releaseDir, required))) {
    console.error(
      `[portable] Missing ${path.join(releaseDir, required)}. ` +
        `Run "pnpm tauri:build:portable" first.`,
    );
    process.exit(1);
  }
}

await rm(portableRoot, { recursive: true, force: true });
await mkdir(folder, { recursive: true });

await cp(path.join(releaseDir, exeName), path.join(folder, appName));
await cp(path.join(releaseDir, sidecarName), path.join(folder, sidecarName), {
  recursive: true,
});
await cp(path.join(releaseDir, "mixchat"), path.join(folder, "mixchat"), {
  recursive: true,
});
await cp(
  path.join(projectRoot, ".env.example"),
  path.join(folder, ".env.example"),
);

await mkdir(path.join(folder, "app-data"), { recursive: true });
await writeFile(
  path.join(folder, "app-data", "README.txt"),
  [
    "This folder is created by Mixchat.",
    "Settings, sessions, chat history, logs, and WebView data are stored here.",
    "Delete it while Mixchat is closed to start fresh.",
    "",
  ].join("\n"),
  "utf8",
);

const instructions = [
  `Mixchat ${packageJson.version} portable`,
  "",
  `1. Keep this folder together. ${appName} needs ${sidecarName} and the mixchat folder.`,
  `2. Run ${appName}.`,
  "3. Data is stored in the app-data folder next to the executable.",
  "4. The app uses http://localhost:3000. Keep that port free.",
  "5. Optional: copy .env.example to .env to override runtime settings.",
  "6. OAuth client IDs are baked into this build at build time.",
];
if (isWindows) {
  instructions.push(
    "7. Requires the Microsoft Edge WebView2 runtime. Windows 11 includes it.",
  );
}
instructions.push("");

await writeFile(
  path.join(folder, "README.txt"),
  instructions.join("\n"),
  "utf8",
);

if (process.platform === "win32") {
  const quote = (value) => value.replace(/'/g, "''");
  execFileSync("powershell.exe", [
    "-NoProfile",
    "-Command",
    `Compress-Archive -LiteralPath '${quote(folder)}' -DestinationPath '${quote(zipPath)}' -Force`,
  ]);
} else {
  try {
    execFileSync("zip", ["-qr", zipName, "Mixchat"], {
      cwd: portableRoot,
      stdio: "inherit",
    });
  } catch {
    console.error(
      '[portable] "zip" is not available. Install zip or zip the folder manually.',
    );
    process.exit(1);
  }
}

const size = await stat(zipPath);
console.log(
  `[portable] ${zipPath} (${(size.size / 1024 / 1024).toFixed(1)} MB)`,
);

import { spawnSync } from "node:child_process";
import { copyFileSync, existsSync, readFileSync, rmSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const projectRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
);
const { version } = JSON.parse(
  readFileSync(path.join(projectRoot, "package.json"), "utf8"),
);
const unpacked = path.join(projectRoot, "release", "win-unpacked");
const zipPath = path.join(
  projectRoot,
  "release",
  `Mixchat-${version}-win-x64-portable.zip`,
);

if (!existsSync(unpacked)) {
  console.error(
    `[zip] ${unpacked} not found — run "electron-builder --win --dir" first.`,
  );
  process.exit(1);
}

// Ship the example config next to the exe.
const envExample = path.join(projectRoot, ".env.example");
if (existsSync(envExample)) {
  copyFileSync(envExample, path.join(unpacked, ".env.example"));
}

rmSync(zipPath, { force: true });
const res = spawnSync("zip", ["-r", "-q", zipPath, "."], {
  cwd: unpacked,
  stdio: "inherit",
});
if (res.error) {
  console.error(
    "[zip] Failed to run `zip` (install it, e.g. `sudo apt install zip`).",
    res.error.message,
  );
  process.exit(1);
}
console.log(`[zip] wrote ${zipPath}`);
process.exit(res.status ?? 1);

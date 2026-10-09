import { readdirSync, rmSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const projectRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
);
const releaseDir = path.join(projectRoot, "release");

// Final artifacts that should remain in release/ after a build. Everything else
// (unpacked dirs, *.blockmap, latest*.yml, builder-debug.yml, stale versions) is
// removed so the folder only holds shippable output.
const KEEP_FILES = [
  /\.deb$/,
  /\.AppImage$/,
  /^Mixchat Setup .+\.exe$/,
  /^Mixchat-.+-win-x64-portable\.zip$/,
];
const KEEP_DIRS = new Set([".icon-ico"]);

const removed = [];
for (const entry of readdirSync(releaseDir)) {
  const full = path.join(releaseDir, entry);
  if (statSync(full).isDirectory()) {
    if (KEEP_DIRS.has(entry)) continue;
  } else if (KEEP_FILES.some((re) => re.test(entry))) {
    continue;
  }
  rmSync(full, { recursive: true, force: true });
  removed.push(entry);
}

console.log(
  `[clean-release] released ${removed.length} entr${
    removed.length === 1 ? "y" : "ies"
  } removed${removed.length ? `: ${removed.join(", ")}` : ""}`,
);

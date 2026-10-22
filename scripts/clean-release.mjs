import { readdirSync, readFileSync, rmSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const projectRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
);
const releaseDir = path.join(projectRoot, "release");
const { version } = JSON.parse(
  readFileSync(path.join(projectRoot, "package.json"), "utf8"),
);

// Final artifacts that should remain in release/ after a build. Everything else
// (unpacked dirs, builder-debug.yml, stale versions) is removed so the folder
// only holds shippable output. The update feed files (latest*.yml and
// *.blockmap) are kept because electron-updater needs them to publish and to
// serve differential downloads.
const KEEP_FILES = [
  /\.deb$/,
  /\.AppImage$/,
  /^Mixchat Setup .+\.exe$/,
  /\.zip$/, // Windows portable zip
  /^latest.*\.yml$/, // latest.yml / latest-linux.yml
  /\.blockmap$/, // differential update data
];
const KEEP_DIRS = new Set([".icon-ico"]);

const removed = [];
for (const entry of readdirSync(releaseDir)) {
  const full = path.join(releaseDir, entry);
  if (statSync(full).isDirectory()) {
    if (KEEP_DIRS.has(entry)) continue;
  } else if (KEEP_FILES.some((re) => re.test(entry))) {
    // Feed files are version-agnostic; everything else must belong to the
    // version currently being released.
    if (/^latest.*\.yml$/.test(entry) || entry.includes(version)) continue;
  }
  rmSync(full, { recursive: true, force: true });
  removed.push(entry);
}

console.log(
  `[clean-release] released ${removed.length} entr${
    removed.length === 1 ? "y" : "ies"
  } removed${removed.length ? `: ${removed.join(", ")}` : ""}`,
);

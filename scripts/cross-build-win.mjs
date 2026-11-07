import { spawnSync } from "node:child_process";
import { mkdirSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const projectRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
);
const target = "x86_64-pc-windows-msvc";
// Pinned by digest (not tag) so a compromised `main` cannot silently
// replace the toolchain that produces Mixchat.exe.
// Digest of messense/cargo-xwin:main as of 2026-11-07.
const image =
  "messense/cargo-xwin@sha256:c6602bccbdf8a72247e986870a4a0044572d7c6dd575240f6ed3af7d3686cdf9";
const cacheRoot = path.join(os.homedir(), ".cache", "mixchat-tauri");

if (process.platform !== "linux") {
  console.error(
    "[cross-win] This script is for Linux. On Windows run: pnpm tauri:build:portable",
  );
  process.exit(1);
}

const dockerCheck = spawnSync("docker", ["--version"], {
  stdio: "ignore",
});
if ((dockerCheck.status ?? 1) !== 0) {
  console.error(
    "[cross-win] Docker is required. Install Docker and make sure the daemon is running.",
  );
  process.exit(1);
}

mkdirSync(path.join(cacheRoot, "xwin"), { recursive: true });
mkdirSync(path.join(cacheRoot, "cargo-registry"), { recursive: true });
mkdirSync(path.join(cacheRoot, "rustup"), { recursive: true });

function run(command, args, env = {}) {
  const result = spawnSync(command, args, {
    cwd: projectRoot,
    stdio: "inherit",
    env: { ...process.env, ...env },
  });
  if ((result.status ?? 1) !== 0) {
    process.exit(result.status ?? 1);
  }
}

run("node", ["scripts/build-mixchat.mjs"]);
run("node", ["scripts/prepare-mixchat.mjs"]);
run("node", ["scripts/prepare-tauri.mjs"]);
run("node", ["scripts/fetch-node.mjs"], {
  TAURI_ENV_TARGET_TRIPLE: target,
});

const containerScript = [
  "set -e",
  "apt-get update -qq",
  "apt-get install -y -qq lld",
  "rustup update stable",
  "rustup default stable",
  `rustup target add ${target}`,
  `cargo xwin build --release --target ${target} || status=$?`,
  "chown -R $(stat -c %u /io):$(stat -c %g /io) " +
    "/io/src-tauri/target /io/src-tauri/Cargo.lock " +
    "/tmp/xwin /usr/local/rustup /usr/local/cargo/registry " +
    "2>/dev/null || true",
  "exit ${status:-0}",
].join("\n");

run("docker", [
  "run",
  "--rm",
  "-v",
  `${projectRoot}:/io`,
  "-v",
  `${path.join(cacheRoot, "xwin")}:/tmp/xwin`,
  "-v",
  `${path.join(cacheRoot, "cargo-registry")}:/usr/local/cargo/registry`,
  "-v",
  `${path.join(cacheRoot, "rustup")}:/usr/local/rustup`,
  "-w",
  "/io/src-tauri",
  "-e",
  "CARGO_HOME=/usr/local/cargo",
  "-e",
  "RUSTUP_HOME=/usr/local/rustup",
  "-e",
  "XWIN_CACHE_DIR=/tmp/xwin",
  "-e",
  "XWIN_CROSS_COMPILER=clang",
  "-e",
  "PATH=/usr/local/cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
  "--entrypoint",
  "sh",
  image,
  "-c",
  containerScript,
]);

run("node", ["scripts/make-portable.mjs", "--target", target]);

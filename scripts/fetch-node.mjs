import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { createWriteStream, existsSync } from "node:fs";
import { chmod, copyFile, mkdir, readFile, readdir, rm, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const projectRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
);
const binariesDir = path.join(projectRoot, "src-tauri", "binaries");
const cacheDir = path.join(projectRoot, "src-tauri", ".cache", "node");
const nodeVersion = process.env.NODE_VERSION || "v26.13.0";
const force = process.argv.includes("--force") || process.env.MIXCHAT_FORCE_NODE === "1";

function hostTriple() {
  if (process.env.TAURI_ENV_TARGET_TRIPLE) return process.env.TAURI_ENV_TARGET_TRIPLE;
  if (process.env.TARGET) return process.env.TARGET;
  try {
    return execFileSync("rustc", ["--print", "host-tuple"], {
      encoding: "utf8",
    }).trim();
  } catch {
    const info = execFileSync("rustc", ["-vV"], { encoding: "utf8" });
    const match = info.match(/^host: (\S+)$/m);
    if (!match) throw new Error("Unable to determine the Rust host target triple");
    return match[1];
  }
}

function nodePlatform(triple) {
  if (triple.startsWith("x86_64-pc-windows")) return { platform: "win-x64", ext: "zip" };
  if (triple.startsWith("aarch64-pc-windows")) return { platform: "win-arm64", ext: "zip" };
  if (triple.startsWith("x86_64-unknown-linux")) return { platform: "linux-x64", ext: "tar.gz" };
  if (triple.startsWith("aarch64-unknown-linux")) return { platform: "linux-arm64", ext: "tar.gz" };
  if (triple.startsWith("x86_64-apple-darwin")) return { platform: "darwin-x64", ext: "tar.gz" };
  if (triple.startsWith("aarch64-apple-darwin")) return { platform: "darwin-arm64", ext: "tar.gz" };
  throw new Error(`Unsupported target triple for the Node sidecar: ${triple}`);
}

async function download(url, destination) {
  const response = await fetch(url);
  if (!response.ok) {
    throw new Error(`Failed to download ${url}: HTTP ${response.status}`);
  }
  await mkdir(path.dirname(destination), { recursive: true });
  const file = createWriteStream(destination);
  const body = response.body;
  if (!body) throw new Error(`Empty response for ${url}`);
  for await (const chunk of body) {
    if (!file.write(chunk)) {
      await new Promise((resolve) => file.once("drain", resolve));
    }
  }
  await new Promise((resolve, reject) => {
    file.end((error) => (error ? reject(error) : resolve()));
  });
}

async function sha256(file) {
  const data = await readFile(file);
  return createHash("sha256").update(data).digest("hex");
}

async function extract(archive, ext, destination) {
  await rm(destination, { recursive: true, force: true });
  await mkdir(destination, { recursive: true });
  if (ext === "tar.gz") {
    execFileSync("tar", ["-xzf", archive, "-C", destination], { stdio: "ignore" });
    return;
  }
  try {
    execFileSync("tar", ["-xf", archive, "-C", destination], { stdio: "ignore" });
  } catch {
    execFileSync("unzip", ["-o", archive, "-d", destination], { stdio: "ignore" });
  }
}

async function findNode(root, extension) {
  const entries = await readdir(root, { withFileTypes: true });
  for (const entry of entries) {
    const full = path.join(root, entry.name);
    if (entry.isDirectory()) {
      const nested = await findNode(full, extension);
      if (nested) return nested;
    } else if (entry.name === `node${extension}`) {
      return full;
    }
  }
  return null;
}

async function main() {
  const triple = hostTriple();
  const { platform, ext } = nodePlatform(triple);
  const extension = platform.startsWith("win") ? ".exe" : "";
  const destination = path.join(binariesDir, `mixchat-node-${triple}${extension}`);

  if (!force && existsSync(destination)) {
    console.log(`[fetch-node] ${destination} already exists, skipping`);
    return;
  }

  await mkdir(binariesDir, { recursive: true });
  await mkdir(cacheDir, { recursive: true });

  const archiveName = `node-${nodeVersion}-${platform}.${ext}`;
  const archive = path.join(cacheDir, archiveName);
  const shasumsFile = path.join(cacheDir, `SHASUMS256-${nodeVersion}.txt`);
  const base = `https://nodejs.org/dist/${nodeVersion}`;

  if (!existsSync(archive)) {
    console.log(`[fetch-node] downloading ${base}/${archiveName}`);
    await download(`${base}/${archiveName}`, archive);
  }
  if (!existsSync(shasumsFile)) {
    await download(`${base}/SHASUMS256.txt`, shasumsFile);
  }

  const shasums = await readFile(shasumsFile, "utf8");
  const expected = shasums
    .split(/\r?\n/)
    .map((line) => line.trim().split(/\s+/))
    .find((parts) => parts[1] === archiveName)?.[0];
  if (!expected) {
    throw new Error(`No checksum found for ${archiveName}`);
  }
  const actual = await sha256(archive);
  if (actual !== expected) {
    throw new Error(`Checksum mismatch for ${archiveName}: ${actual} != ${expected}`);
  }

  const extracted = path.join(cacheDir, `node-${nodeVersion}-${platform}`);
  let source = existsSync(extracted) ? await findNode(extracted, extension) : null;
  if (!source) {
    console.log(`[fetch-node] extracting ${archiveName}`);
    await extract(archive, ext, extracted);
    source = await findNode(extracted, extension);
  }
  if (!source) {
    throw new Error(`Node binary not found in ${extracted}`);
  }

  await copyFile(source, destination);
  if (!platform.startsWith("win")) {
    await chmod(destination, 0o755);
  }
  await writeFile(
    path.join(binariesDir, `mixchat-node${extension}.version`),
    `${nodeVersion}\n`,
    "utf8",
  );
  console.log(`[fetch-node] ${destination} (${nodeVersion})`);
}

await main();

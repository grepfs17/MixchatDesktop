import { existsSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";

const args = process.argv.slice(2);

function value(flag) {
  const index = args.indexOf(flag);
  if (index === -1) return null;
  return args[index + 1] ?? null;
}

const dist = path.resolve(value("--dist") ?? "dist");
const tag = value("--tag") ?? process.env.TAG ?? process.env.GITHUB_REF_NAME;
const repo = value("--repo") ?? process.env.REPO ?? process.env.GITHUB_REPOSITORY;
const packageJson = JSON.parse(readFileSync("package.json", "utf8"));
const version = (value("--version") ?? process.env.VERSION ?? packageJson.version).replace(
  /^v/,
  "",
);

if (!tag || !repo) {
  console.error(
    "[latest-json] --tag and --repo (or GITHUB_REF_NAME/GITHUB_REPOSITORY) are required",
  );
  process.exit(1);
}
if (!existsSync(dist)) {
  console.error(`[latest-json] ${dist} does not exist`);
  process.exit(1);
}

const files = readdirSync(dist);

function findAsset(suffix) {
  const name = files.find((entry) => entry.endsWith(suffix));
  if (!name) {
    console.error(`[latest-json] no *${suffix} in ${dist}`);
    process.exit(1);
  }
  return name;
}

function signature(name) {
  const sigPath = path.join(dist, `${name}.sig`);
  if (!existsSync(sigPath)) {
    console.error(`[latest-json] missing signature ${sigPath}`);
    process.exit(1);
  }
  const sig = readFileSync(sigPath, "utf8").trim();
  if (!sig) {
    console.error(`[latest-json] empty signature ${sigPath}`);
    process.exit(1);
  }
  return sig;
}

function assetUrl(name) {
  return `https://github.com/${repo}/releases/download/${encodeURIComponent(tag)}/${encodeURIComponent(name)}`;
}

const windows = findAsset("-setup.exe");
const portable = findAsset("-portable.zip");
const linux = findAsset(".AppImage");

const manifest = {
  version,
  notes: `Release notes: https://github.com/${repo}/releases/tag/${encodeURIComponent(tag)}`,
  pub_date: new Date().toISOString().replace(/\.\d{3}Z$/, "Z"),
  platforms: {
    "windows-x86_64": {
      signature: signature(windows),
      url: assetUrl(windows),
    },
    "windows-x86_64-portable": {
      signature: signature(portable),
      url: assetUrl(portable),
    },
    "linux-x86_64": {
      signature: signature(linux),
      url: assetUrl(linux),
    },
  },
};

writeFileSync(
  path.join(dist, "latest.json"),
  `${JSON.stringify(manifest, null, 2)}\n`,
);
console.log(
  `[latest-json] wrote latest.json for ${windows}, ${portable}, and ${linux} (version ${version})`,
);

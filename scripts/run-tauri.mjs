import { spawn } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

const projectRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
);
const mode = process.argv[2] || "local";
const pnpm = process.platform === "win32" ? "pnpm.cmd" : "pnpm";

const child = spawn(pnpm, ["exec", "tauri", "dev"], {
  cwd: projectRoot,
  stdio: "inherit",
  env: { ...process.env, MIXCHAT_MODE: mode },
});

child.on("exit", (code, signal) => {
  if (signal) {
    process.kill(process.pid, signal);
  } else {
    process.exit(code ?? 0);
  }
});

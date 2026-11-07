import { spawn } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { pnpmCommand } from "./pnpm.mjs";

const projectRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
);
const mode = process.argv[2] || "local";
const { command, argv } = pnpmCommand(["exec", "tauri", "dev"]);

const child = spawn(command, argv, {
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

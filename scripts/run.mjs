import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import electronPath from "electron";
import { parseEnvFile } from "../src/main/mixchat-server.mjs";

const projectRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
);
const envFile = path.join(projectRoot, ".env");
const fileEnv = existsSync(envFile) ? parseEnvFile(envFile) : {};

const mode = process.argv[2] || "dev";
const passthrough = process.argv.slice(3);

// Merge .env first, then let already-exported variables win.
const env = { ...fileEnv, ...process.env, MIXCHAT_MODE: mode };

// Linux workaround for environments where chrome-sandbox is not setuid root:
// set MIXCHAT_NO_SANDBOX=1 in .env or the shell.
if (env.MIXCHAT_NO_SANDBOX === "1" && process.platform === "linux") {
  passthrough.push("--no-sandbox");
}

const child = spawn(electronPath, [".", ...passthrough], {
  stdio: "inherit",
  env,
});

child.on("exit", (code, signal) => {
  if (signal) process.kill(process.pid, signal);
  else process.exit(code ?? 0);
});

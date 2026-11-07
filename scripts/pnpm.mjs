import { spawnSync } from "node:child_process";

/**
 * Resolve how to launch pnpm for a given argument list. Node >= 18.20.2
 * refuses to spawn .cmd/.bat files directly (CVE-2024-27980), so on Windows
 * pnpm.cmd must be invoked through cmd.exe; child_process quotes each
 * argument as usual.
 */
export function pnpmCommand(args) {
  if (process.platform === "win32") {
    return { command: "cmd.exe", argv: ["/d", "/c", "pnpm.cmd", ...args] };
  }
  return { command: "pnpm", argv: args };
}

/**
 * Spawn pnpm synchronously with stdio inherited. Returns the spawnSync
 * result (callers check `status`); a failed start is reported on stderr
 * and surfaces as `status === null`.
 */
export function runPnpm(args, options = {}) {
  const { command, argv } = pnpmCommand(args);
  const result = spawnSync(command, argv, {
    cwd: options.cwd,
    env: options.env ?? process.env,
    stdio: "inherit",
  });
  if (result.error) {
    console.error(`[pnpm] failed to start: ${result.error.message}`);
  }
  return result;
}

import { app, BrowserWindow, Menu, shell, dialog, nativeTheme } from "electron";
import { existsSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  findEnvFile,
  mixchatServerEntry,
  parseEnvFile,
  startMixchatServer,
  waitForServer,
} from "./mixchat-server.mjs";

const __dirname = path.dirname(fileURLToPath(import.meta.url));

// Load desktop-level configuration (MIXCHAT_* keys only) from a .env file
// without overriding variables that were already exported in the environment.
const CONFIG_ENV_FILE = findEnvFile([
  process.env.MIXCHAT_CONFIG,
  path.join(app.getAppPath(), ".env"),
  path.join(app.getPath("userData"), ".env"),
]);
const configEnv = CONFIG_ENV_FILE ? parseEnvFile(CONFIG_ENV_FILE) : {};
for (const [key, value] of Object.entries(configEnv)) {
  if (key.startsWith("MIXCHAT_") && process.env[key] === undefined) {
    process.env[key] = value;
  }
}

const MODE = process.env.MIXCHAT_MODE || (app.isPackaged ? "packaged" : "local");
const PORT = Number(process.env.MIXCHAT_PORT || 3000);
const HOST = process.env.MIXCHAT_HOST || "127.0.0.1";

// Force dark for every web contents (main window + OAuth popups). Without this
// Chromium follows the OS scheme, so Google/YouTube/Twitch auth pages render
// light even though the app itself is dark.
nativeTheme.themeSource = "dark";

let serverChild = null;
let mainWindow = null;

const gotLock = app.requestSingleInstanceLock();
if (!gotLock) {
  app.quit();
} else {
  app.on("second-instance", () => {
    if (mainWindow) {
      if (mainWindow.isMinimized()) mainWindow.restore();
      mainWindow.focus();
    }
  });
}

function mixchatRoot() {
  if (process.env.MIXCHAT_DIR) return path.resolve(process.env.MIXCHAT_DIR);
  if (MODE === "packaged") return path.join(app.getAppPath(), "mixchat");
  const prepared = path.join(app.getAppPath(), "build", "mixchat");
  if (existsSync(path.join(prepared, "dist", "server", "entry.mjs"))) {
    return prepared;
  }
  return path.resolve(app.getAppPath(), "..", "Mixchat");
}

function resolveEnvFile(root) {
  return findEnvFile([
    process.env.MIXCHAT_ENV_FILE,
    path.join(root, ".env"),
    path.join(app.getPath("userData"), ".env"),
    path.join(path.dirname(app.getPath("exe")), ".env"),
  ]);
}

async function resolveTarget() {
  if (MODE === "dev") {
    const url = process.env.MIXCHAT_URL || `http://localhost:${PORT}`;
    console.log(`[desktop] dev mode: waiting for external server at ${url}`);
    await waitForServer(url, 60000);
    return { url, external: true };
  }

  const root = mixchatRoot();
  const entry = mixchatServerEntry(root);
  const envFile = resolveEnvFile(root);
  // When packaged, run from a writable directory so relative paths (session
  // storage, data dirs) don't land in a read-only install/resources folder.
  const cwd = MODE === "packaged" ? app.getPath("userData") : root;
  console.log(`[desktop] mode=${MODE} root=${root}`);
  console.log(`[desktop] env file: ${envFile ?? "(none found)"}`);
  console.log(`[desktop] cwd=${cwd}`);

  // Desktop .env is applied last so it can override Mixchat's own .env for the
  // runtime vars (ORIGIN, YT_*, storage paths).
  const env = {
    ...process.env,
    ...(envFile ? parseEnvFile(envFile) : {}),
    ...configEnv,
    PORT: String(PORT),
    HOST,
    NODE_ENV: "production",
  };

  if (MODE === "packaged") {
    env.MIXCHAT_SONG_QUEUE_DIR = path.join(app.getPath("userData"), "song-queue");
    env.MIXCHAT_WHEEL_DIR = path.join(app.getPath("userData"), "wheel");
  }

  serverChild = startMixchatServer({
    root,
    entry,
    env,
    cwd,
    onLog: (line) => console.log(line),
  });

  // Bind address (HOST) is only where the server listens. The window must load
  // the SAME origin the OAuth redirect URIs use (localhost), otherwise the
  // host-scoped oauth_state cookie isn't sent back on the callback.
  const readyUrl = `http://${HOST}:${PORT}`;
  const loadUrl = process.env.MIXCHAT_URL || `http://localhost:${PORT}`;
  console.log(`[desktop] readiness=${readyUrl} load=${loadUrl}`);
  await waitForServer(readyUrl, 60000, serverChild);
  return { url: loadUrl, external: false };
}

const AUTH_HOSTS = [
  "accounts.google.com",
  "accounts.youtube.com",
  "google.com",
  "gstatic.com",
  "twitch.tv",
  "id.twitch.tv",
  "youtube.com",
  "youtu.be",
];

function isInternalUrl(rawUrl, appOrigin) {
  try {
    const { origin } = new URL(rawUrl);
    return origin === appOrigin;
  } catch {
    return false;
  }
}

function isAuthUrl(rawUrl) {
  try {
    const { hostname } = new URL(rawUrl);
    if (hostname === "localhost" || hostname === "127.0.0.1") return true;
    return AUTH_HOSTS.some((h) => hostname === h || hostname.endsWith(`.${h}`));
  } catch {
    return false;
  }
}

function attachSecurity(window, appOrigin) {
  window.webContents.setWindowOpenHandler(({ url }) => {
    if (isAuthUrl(url)) {
      return {
        action: "allow",
        overrideBrowserWindowOptions: {
          width: 600,
          height: 760,
          autoHideMenuBar: true,
          backgroundColor: "#0e0e10",
          webPreferences: {
            contextIsolation: true,
            nodeIntegration: false,
            sandbox: true,
          },
        },
      };
    }
    shell.openExternal(url);
    return { action: "deny" };
  });

  window.webContents.on("will-navigate", (event, url) => {
    if (!isInternalUrl(url, appOrigin)) {
      event.preventDefault();
      shell.openExternal(url);
    }
  });
}

function buildMenu(window) {
  const template = [
    {
      label: "File",
      submenu: [{ role: "quit" }],
    },
    {
      label: "View",
      submenu: [
        { role: "reload" },
        { role: "forceReload" },
        { role: "toggleDevTools" },
        { type: "separator" },
        { role: "resetZoom" },
        { role: "zoomIn" },
        { role: "zoomOut" },
        { type: "separator" },
        { role: "togglefullscreen" },
      ],
    },
    {
      label: "Window",
      submenu: [{ role: "minimize" }, { role: "close" }],
    },
  ];
  Menu.setApplicationMenu(Menu.buildFromTemplate(template));
}

async function createWindow() {
  let target;
  try {
    target = await resolveTarget();
  } catch (error) {
    dialog.showErrorBox("Mixchat failed to start", String(error?.message ?? error));
    app.quit();
    return;
  }

  const appOrigin = new URL(target.url).origin;

  mainWindow = new BrowserWindow({
    width: 1600,
    height: 900,
    minWidth: 900,
    minHeight: 600,
    backgroundColor: "#0e0e10",
    autoHideMenuBar: true,
    title: "Mixchat",
    webPreferences: {
      preload: path.join(__dirname, "..", "preload", "index.mjs"),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
    },
  });

  attachSecurity(mainWindow, appOrigin);
  buildMenu(mainWindow);

  mainWindow.on("closed", () => {
    mainWindow = null;
  });

  if (!MODE.startsWith("dev")) {
    mainWindow.webContents.on("did-fail-load", (_e, code, desc, url) => {
      if (code !== -3) console.error(`[desktop] load failed (${code} ${desc}) ${url}`);
    });
  }

  await mainWindow.loadURL(target.url);
}

app.whenReady().then(createWindow);

app.on("activate", () => {
  if (BrowserWindow.getAllWindows().length === 0) createWindow();
});

app.on("window-all-closed", () => {
  if (process.platform !== "darwin") app.quit();
});

app.on("before-quit", () => {
  if (serverChild && serverChild.exitCode === null) {
    serverChild.kill("SIGTERM");
  }
});

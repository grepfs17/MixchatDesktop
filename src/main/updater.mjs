import { app, dialog } from "electron";
import updaterPkg from "electron-updater";

const { autoUpdater } = updaterPkg;

/**
 * Wire electron-updater into the packaged app. No-op in development or when the
 * build has no update metadata (app-update.yml), so `pnpm dev`/`pnpm start`
 * never hit the network.
 *
 * Flow: check on launch -> ask before downloading -> ask before restarting.
 * Every failure is logged rather than thrown: an unreachable update server must
 * never stop the app from starting.
 */
export function setupAutoUpdater({ getWindow, logger = console }) {
  if (!app.isPackaged) return;

  autoUpdater.logger = logger;
  autoUpdater.autoDownload = false;
  autoUpdater.autoInstallOnAppQuit = true;

  autoUpdater.on("error", (error) => {
    logger.error(`[updater] ${error?.message ?? error}`);
  });

  autoUpdater.on("update-available", (info) => {
    void promptDownload(info, getWindow);
  });

  autoUpdater.on("update-downloaded", (info) => {
    void promptInstall(info, getWindow);
  });

  void autoUpdater.checkForUpdates().catch((error) => {
    logger.error(`[updater] check failed: ${error?.message ?? error}`);
  });
}

/** Manual "Check for Updates" menu action. */
export function checkForUpdates({ logger = console } = {}) {
  if (!app.isPackaged) {
    dialog.showMessageBox({
      type: "info",
      message: "Updates are only available in packaged builds.",
    });
    return;
  }
  void autoUpdater.checkForUpdates().catch((error) => {
    logger.error(`[updater] check failed: ${error?.message ?? error}`);
  });
}

async function promptDownload(info, getWindow) {
  const { response } = await dialog.showMessageBox(getWindow?.(), {
    type: "info",
    buttons: ["Download", "Later"],
    defaultId: 0,
    cancelId: 1,
    title: "Update available",
    message: `Mixchat ${info?.version ?? ""} is available.`,
    detail: "Download it now? The update installs after you restart the app.",
  });
  if (response === 0) {
    await autoUpdater.downloadUpdate().catch(() => {});
  }
}

async function promptInstall(info, getWindow) {
  const { response } = await dialog.showMessageBox(getWindow?.(), {
    type: "info",
    buttons: ["Restart now", "Later"],
    defaultId: 0,
    cancelId: 1,
    title: "Update ready",
    message: `Mixchat ${info?.version ?? ""} has been downloaded.`,
    detail: "Restart to install it.",
  });
  if (response === 0) {
    autoUpdater.quitAndInstall();
  }
}

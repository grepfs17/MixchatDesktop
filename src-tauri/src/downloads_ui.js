(() => {
  if (window.__MIXCHAT_DOWNLOADS__) return;

  const views = new Map();
  const items = new Map();
  const speeds = new Map();
  let host = null;
  let list = null;
  let live = null;

  const css = `
.mxc-downloads,
.mxc-downloads * { box-sizing: border-box; }
.mxc-downloads {
  position: fixed;
  right: 20px;
  bottom: 20px;
  z-index: 2147483000;
  display: flex;
  flex-direction: column;
  gap: 8px;
  width: 340px;
  max-width: calc(100vw - 40px);
  font-family: var(--font-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif);
  font-size: 13px;
  line-height: 1.4;
  color: var(--color-text-primary, #f1f5f9);
  pointer-events: none;
}
.mxc-downloads[hidden] { display: none; }
.mxc-download {
  pointer-events: auto;
  padding: 10px 12px;
  background: var(--color-bg-secondary, #141417);
  border: 1px solid var(--color-border-medium, #2a2a32);
  border-radius: var(--radius-lg, 0);
  box-shadow: var(--shadow-md, 0 4px 16px rgba(0, 0, 0, 0.35));
}
.mxc-download[data-state="error"] { border-color: var(--color-notification-error, #ef4444); }
.mxc-download-head { display: flex; align-items: center; gap: 8px; }
.mxc-download-name {
  flex: 1 1 auto;
  min-width: 0;
  font-weight: 600;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.mxc-download-close {
  flex: 0 0 auto;
  padding: 0 2px;
  border: 0;
  background: transparent;
  color: var(--color-text-secondary, #94a3b8);
  font: inherit;
  font-size: 18px;
  line-height: 1;
  cursor: pointer;
}
.mxc-download-close:hover { color: var(--color-text-primary, #f1f5f9); }
.mxc-download-bar {
  position: relative;
  height: 4px;
  margin: 8px 0 7px;
  background: var(--color-bg-hover, #292930);
  overflow: hidden;
}
.mxc-download-fill {
  height: 100%;
  width: 0;
  background: var(--color-accent, #9e4073);
  transition: width 0.2s linear;
}
.mxc-download[data-state="done"] .mxc-download-fill {
  background: var(--color-notification-success, #10b981);
}
.mxc-download-bar.is-indeterminate .mxc-download-fill {
  width: 30%;
  animation: mxc-download-slide 1.1s ease-in-out infinite;
}
.mxc-download-bar.is-hidden { visibility: hidden; }
@keyframes mxc-download-slide {
  from { transform: translateX(-110%); }
  to { transform: translateX(360%); }
}
.mxc-download-foot {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
}
.mxc-download-status {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--color-text-secondary, #94a3b8);
  font-size: 12px;
  font-variant-numeric: tabular-nums;
}
.mxc-download[data-state="error"] .mxc-download-status { color: #fca5a5; }
.mxc-download[data-state="done"] .mxc-download-status { color: #6ee7b7; }
.mxc-download-action {
  flex: 0 0 auto;
  padding: 4px 9px;
  border: 1px solid var(--color-border-light, #3d3d47);
  border-radius: var(--radius-sm, 0);
  background: var(--color-bg-tertiary, #1e1e23);
  color: var(--color-text-primary, #f1f5f9);
  font: inherit;
  font-size: 12px;
  cursor: pointer;
}
.mxc-download-action:hover { background: var(--color-bg-hover, #292930); }
.mxc-download-close:focus-visible,
.mxc-download-action:focus-visible {
  outline: 2px solid var(--color-accent, #9e4073);
  outline-offset: 2px;
}
.mxc-downloads-live {
  position: absolute;
  width: 1px;
  height: 1px;
  padding: 0;
  margin: -1px;
  overflow: hidden;
  clip: rect(0 0 0 0);
  white-space: nowrap;
  border: 0;
}
@media (prefers-reduced-motion: reduce) {
  .mxc-download-fill { transition: none; }
  .mxc-download-bar.is-indeterminate .mxc-download-fill {
    width: 100%;
    opacity: 0.5;
    animation: none;
  }
}
`;

  function invoke(command, args) {
    const internals = window.__TAURI_INTERNALS__;
    if (!internals || typeof internals.invoke !== "function") {
      return Promise.resolve(undefined);
    }
    return internals.invoke(command, args).catch(() => undefined);
  }

  function el(tag, className, text) {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (text != null) node.textContent = text;
    return node;
  }

  function idNumber(id) {
    const value = Number(String(id).replace(/\D+/g, ""));
    return Number.isFinite(value) ? value : 0;
  }

  function isActive(view) {
    return Boolean(view) && (view.state === "preparing" || view.state === "running");
  }

  function formatBytes(bytes) {
    if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
    const units = ["B", "KB", "MB", "GB", "TB"];
    let value = bytes;
    let unit = 0;
    while (value >= 1024 && unit < units.length - 1) {
      value /= 1024;
      unit += 1;
    }
    const digits = value >= 100 || unit === 0 ? 0 : 1;
    return value.toFixed(digits) + " " + units[unit];
  }

  function speedFor(view) {
    if (view.state !== "running" || !view.received) return 0;
    const now = performance.now();
    const previous = speeds.get(view.id);
    if (!previous) {
      speeds.set(view.id, { received: view.received, at: now, bps: 0 });
      return 0;
    }
    if (previous.received === view.received) return previous.bps;
    const elapsed = (now - previous.at) / 1000;
    if (elapsed < 0.4) return previous.bps;
    const bps = Math.max(0, (view.received - previous.received) / elapsed);
    speeds.set(view.id, { received: view.received, at: now, bps });
    return bps;
  }

  function announce(message) {
    if (!live) return;
    live.textContent = "";
    window.setTimeout(() => {
      if (live) live.textContent = message;
    }, 30);
  }

  function syncVisibility() {
    if (host) host.hidden = items.size === 0;
  }

  function mount() {
    if (host || !document.body) return;
    const style = document.createElement("style");
    style.textContent = css;
    (document.head || document.documentElement).appendChild(style);

    host = el("section", "mxc-downloads");
    host.setAttribute("aria-label", "Downloads");
    list = el("div", "mxc-downloads-list");
    live = el("div", "mxc-downloads-live");
    live.setAttribute("role", "status");
    live.setAttribute("aria-live", "polite");
    host.append(list, live);
    document.body.appendChild(host);

    const ordered = Array.from(views.values()).sort(
      (a, b) => idNumber(a.id) - idNumber(b.id),
    );
    for (const view of ordered) applyView(view);
    syncVisibility();
  }

  function ensureItem(view) {
    const existing = items.get(view.id);
    if (existing) return existing;

    const root = el("article", "mxc-download");
    const head = el("div", "mxc-download-head");
    const name = el("span", "mxc-download-name");
    const close = el("button", "mxc-download-close");
    close.type = "button";
    close.textContent = "\u00d7";

    const bar = el("div", "mxc-download-bar");
    const fill = el("div", "mxc-download-fill");
    bar.appendChild(fill);

    const foot = el("div", "mxc-download-foot");
    const status = el("span", "mxc-download-status");
    const action = el("button", "mxc-download-action");
    action.type = "button";

    head.append(name, close);
    foot.append(status, action);
    root.append(head, bar, foot);

    const item = { root, name, close, bar, fill, status, action, view: null };

    close.addEventListener("click", () => {
      const view = item.view;
      if (!view) return;
      if (isActive(view)) {
        invoke("download_cancel", { id: view.id });
      } else {
        invoke("download_dismiss", { id: view.id });
        removeItem(view.id);
      }
    });

    action.addEventListener("click", () => {
      const view = item.view;
      if (!view) return;
      if (view.state === "done") {
        invoke("download_reveal", { id: view.id });
      } else if (view.state === "error" || view.state === "cancelled") {
        invoke("download_dismiss", { id: view.id });
        removeItem(view.id);
      } else {
        invoke("download_cancel", { id: view.id });
      }
    });

    list.prepend(root);
    items.set(view.id, item);
    return item;
  }

  function applyView(view) {
    if (!view || !view.id) return;
    views.set(view.id, view);
    if (!host) return;

    const item = ensureItem(view);
    const previousState = item.view ? item.view.state : null;
    item.view = view;

    item.root.dataset.state = view.state;
    item.name.textContent = view.filename || "Download";
    item.name.title = view.filename || "";
    item.status.title = view.state === "done" && view.path ? view.path : "";

    const active = isActive(view);
    const label = active ? "Cancel download" : "Dismiss download";
    item.close.setAttribute("aria-label", label);
    item.close.title = label;

    const fraction =
      view.total && view.total > 0
        ? Math.min(1, view.received / view.total)
        : null;

    if (view.state === "done") {
      item.bar.className = "mxc-download-bar is-determinate";
      item.fill.style.width = "100%";
    } else if (fraction !== null) {
      item.bar.className = "mxc-download-bar is-determinate";
      item.fill.style.width = (fraction * 100).toFixed(1) + "%";
    } else if (active) {
      item.bar.className = "mxc-download-bar is-indeterminate";
      item.fill.style.width = "";
    } else {
      item.bar.className = "mxc-download-bar is-hidden";
      item.fill.style.width = "0";
    }

    if (view.state === "preparing") {
      item.status.textContent = "Preparing download";
    } else if (view.state === "running") {
      if (!view.received) {
        item.status.textContent = "Starting download";
      } else {
        const bps = speedFor(view);
        const parts = [];
        if (fraction !== null) parts.push(Math.floor(fraction * 100) + "%");
        const received =
          fraction !== null && view.total
            ? formatBytes(view.received) + " of " + formatBytes(view.total)
            : formatBytes(view.received);
        parts.push(received);
        if (bps > 0) parts.push(formatBytes(bps) + "/s");
        item.status.textContent = parts.join(" \u00b7 ");
      }
    } else if (view.state === "done") {
      item.status.textContent = "Saved to Downloads";
    } else if (view.state === "cancelled") {
      item.status.textContent = "Cancelled";
    } else {
      item.status.textContent = view.error || "Download failed";
    }

    if (view.state === "done") {
      item.action.textContent = "Show in folder";
    } else if (view.state === "error" || view.state === "cancelled") {
      item.action.textContent = "Dismiss";
    } else {
      item.action.textContent = "Cancel";
    }

    if (live && previousState && previousState !== view.state) {
      if (view.state === "done") {
        announce("Download complete: " + (view.filename || "file"));
      } else if (view.state === "error") {
        announce("Download failed: " + (view.filename || "file"));
      } else if (view.state === "cancelled") {
        announce("Download cancelled: " + (view.filename || "file"));
      }
    }

    syncVisibility();
  }

  function removeItem(id) {
    const item = items.get(id);
    if (item) {
      item.root.remove();
      items.delete(id);
    }
    views.delete(id);
    speeds.delete(id);
    syncVisibility();
  }

  async function refresh() {
    const snapshot = await invoke("download_snapshot");
    if (!Array.isArray(snapshot)) return;
    const ordered = snapshot
      .slice()
      .sort((a, b) => idNumber(a.id) - idNumber(b.id));
    for (const view of ordered) {
      if (!views.has(view.id)) applyView(view);
    }
  }

  window.__MIXCHAT_DOWNLOADS__ = {
    update(view) {
      applyView(view);
    },
    remove(id) {
      removeItem(id);
    },
  };

  // Mixchat starts downloads with an anchor that has the download attribute.
  // Cancel same-origin navigations here so the shell streams the file itself,
  // which keeps the webview from requesting it twice and gives real progress.
  document.addEventListener(
    "click",
    (event) => {
      if (event.defaultPrevented) return;
      const target = event.target;
      const anchor =
        target instanceof Element ? target.closest("a[download][href]") : null;
      if (!anchor) return;
      let url;
      try {
        url = new URL(anchor.href, window.location.href);
      } catch {
        return;
      }
      if (url.origin !== window.location.origin) return;
      if (url.protocol !== "http:" && url.protocol !== "https:") return;
      event.preventDefault();
      event.stopImmediatePropagation();
      const name = anchor.getAttribute("download");
      invoke("download_start", { url: url.href, filename: name || null });
    },
    true,
  );

  if (document.readyState === "loading") {
    document.addEventListener(
      "DOMContentLoaded",
      () => {
        mount();
        refresh();
      },
      { once: true },
    );
  } else {
    mount();
    refresh();
  }
})();

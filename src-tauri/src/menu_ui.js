(() => {
  if (window.__MIXCHAT_MENU__) return;

  // Copy, cut, and select all run in-page because they act on this webview's
  // own selection, not on shell state. Paste is the exception: webviews refuse
  // script-initiated clipboard reads, so it asks the shell for the text.
  const EDIT_ACTIONS = {
    copy: "copy",
    cut: "cut",
    "select-all": "selectAll",
  };

  const css = `
.mxc-menu-overlay {
  position: fixed;
  inset: 0;
  z-index: 2147483647;
}
.mxc-menu {
  position: absolute;
  min-width: 220px;
  max-width: min(340px, calc(100vw - 16px));
  max-height: calc(100vh - 16px);
  overflow-y: auto;
  padding: 6px;
  background: var(--color-bg-secondary, #141417);
  border: 1px solid var(--color-border-medium, #2a2a32);
  border-radius: 8px;
  box-shadow: 0 12px 32px rgba(0, 0, 0, 0.5);
  color: var(--color-text-primary, #f1f5f9);
  font-family: system-ui, -apple-system, "Segoe UI", sans-serif;
  font-size: 13px;
  line-height: 1.4;
  user-select: none;
}
.mxc-menu-item {
  display: flex;
  align-items: center;
  gap: 28px;
  width: 100%;
  padding: 7px 10px;
  border: 0;
  border-radius: 6px;
  background: transparent;
  color: inherit;
  font: inherit;
  text-align: left;
  cursor: default;
}
.mxc-menu-item:hover,
.mxc-menu-item:focus-visible {
  background: var(--color-bg-hover, #292930);
  outline: none;
}
.mxc-menu-label {
  flex: 1 1 auto;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.mxc-menu-shortcut {
  flex: 0 0 auto;
  color: var(--color-text-secondary, #8b8b95);
  font-size: 12px;
}
.mxc-menu-sep {
  height: 1px;
  margin: 5px 8px;
  background: var(--color-border-medium, #2a2a32);
}
`;

  let overlay = null;
  let menu = null;
  // Focused element before the menu took focus, put back on dismissal.
  let previousFocus = null;

  function invoke(command, args) {
    const internals = window.__TAURI_INTERNALS__;
    if (!internals || typeof internals.invoke !== "function") {
      return Promise.resolve(undefined);
    }
    return internals.invoke(command, args).catch(() => undefined);
  }

  function close(restoreFocus) {
    if (!overlay) return;
    overlay.style.display = "none";
    if (restoreFocus && previousFocus && previousFocus.isConnected) {
      try {
        previousFocus.focus();
      } catch (error) {
        // A detached or unfocusable element simply keeps no focus.
      }
    }
    previousFocus = null;
  }

  // Text input of the right shape, as opposed to a checkbox or a slider.
  function isTextInput(element) {
    if (!element || element.tagName !== "INPUT") return false;
    const type = (element.type || "text").toLowerCase();
    return ["text", "search", "url", "tel", "email", "password", "number"].includes(type);
  }

  // execCommand covers contenteditable and most inputs in one call and fires
  // the input event itself. setRangeText is the fallback for engines that only
  // allow insertText inside an editable region.
  function insertText(text) {
    try {
      if (document.execCommand("insertText", false, text)) return true;
    } catch (error) {
      // Fall through to the direct edit below.
    }
    const element = document.activeElement;
    if (!element || (element.tagName !== "TEXTAREA" && !isTextInput(element))) return false;
    try {
      const start = element.selectionStart ?? element.value.length;
      const end = element.selectionEnd ?? start;
      element.setRangeText(text, start, end, "end");
      element.dispatchEvent(new Event("input", { bubbles: true }));
      return true;
    } catch (error) {
      return false;
    }
  }

  async function paste() {
    const text = await invoke("read_clipboard_text");
    if (typeof text === "string" && text) insertText(text);
  }

  function activate(id) {
    // Restore focus first so copy and cut read the selection the menu opened
    // over, and so paste lands in the element the user was editing.
    close(true);
    if (EDIT_ACTIONS[id]) {
      try {
        document.execCommand(EDIT_ACTIONS[id]);
      } catch (error) {
        // Ignore unsupported clipboard commands.
      }
      return;
    }
    if (id === "paste") {
      paste();
      return;
    }
    invoke("menu_action", { id });
  }

  function build(items) {
    menu.textContent = "";
    for (const item of items) {
      if (item.type === "separator") {
        const sep = document.createElement("div");
        sep.className = "mxc-menu-sep";
        sep.setAttribute("role", "separator");
        menu.appendChild(sep);
        continue;
      }

      const button = document.createElement("button");
      button.type = "button";
      button.className = "mxc-menu-item";
      button.dataset.id = item.id;
      button.setAttribute("role", "menuitem");

      const label = document.createElement("span");
      label.className = "mxc-menu-label";
      label.textContent = item.label;
      button.appendChild(label);

      if (item.shortcut) {
        const shortcut = document.createElement("span");
        shortcut.className = "mxc-menu-shortcut";
        shortcut.textContent = item.shortcut;
        button.appendChild(shortcut);
      }

      button.addEventListener("click", () => activate(item.id));
      menu.appendChild(button);
    }
  }

  function position(x, y) {
    const width = menu.offsetWidth;
    const height = menu.offsetHeight;
    const viewportWidth = window.innerWidth;
    const viewportHeight = window.innerHeight;
    const left = Math.min(Math.max(0, x), Math.max(0, viewportWidth - width));
    const top = Math.min(Math.max(0, y), Math.max(0, viewportHeight - height));
    menu.style.left = left + "px";
    menu.style.top = top + "px";
  }

  function menuItems() {
    return Array.from(menu.querySelectorAll(".mxc-menu-item"));
  }

  function focusRelative(step) {
    const list = menuItems();
    if (!list.length) return;
    let index = list.indexOf(document.activeElement);
    if (index < 0) index = step > 0 ? list.length - 1 : 0;
    list[(index + step + list.length) % list.length].focus();
  }

  function focusEdge(target) {
    const list = menuItems();
    const element = target === "last" ? list[list.length - 1] : list[0];
    if (element) element.focus();
  }

  function open(payload) {
    if (!document.body) return;
    const wasOpen = !!overlay && overlay.style.display !== "none";
    if (!overlay) {
      const style = document.createElement("style");
      style.textContent = css;
      (document.head || document.documentElement).appendChild(style);

      overlay = document.createElement("div");
      overlay.className = "mxc-menu-overlay";
      overlay.addEventListener("click", (event) => {
        if (event.target === overlay) close(true);
      });

      menu = document.createElement("div");
      menu.className = "mxc-menu";
      menu.setAttribute("role", "menu");
      overlay.appendChild(menu);
      document.body.appendChild(overlay);
    }

    if (!wasOpen) {
      const active = document.activeElement;
      previousFocus = active && active !== document.body ? active : null;
    }

    build(payload.items || []);
    overlay.style.display = "";
    position(payload.x || 0, payload.y || 0);

    const first = menu.querySelector(".mxc-menu-item");
    if (first) first.focus();
  }

  function onKeydown(event) {
    if (!overlay || overlay.style.display === "none") return;
    // The open menu is modal: page shortcuts must not fire behind it.
    event.stopPropagation();

    if (event.key === "Escape") {
      event.preventDefault();
      close(true);
      return;
    }
    if (event.key === "ArrowDown" || (event.key === "Tab" && !event.shiftKey)) {
      event.preventDefault();
      focusRelative(1);
      return;
    }
    if (event.key === "ArrowUp" || (event.key === "Tab" && event.shiftKey)) {
      event.preventDefault();
      focusRelative(-1);
      return;
    }
    if (event.key === "Home") {
      event.preventDefault();
      focusEdge("first");
      return;
    }
    if (event.key === "End") {
      event.preventDefault();
      focusEdge("last");
      return;
    }
    // Enter and Space reach the focused item as its default action, and
    // modifier combinations stay in the menu, so nothing else needs a guard.
  }

  document.addEventListener("keydown", onKeydown, true);
  window.addEventListener("blur", () => close());
  window.addEventListener("resize", () => close());
  window.addEventListener("scroll", () => close(), true);

  window.__MIXCHAT_MENU__ = { open, close };
})();

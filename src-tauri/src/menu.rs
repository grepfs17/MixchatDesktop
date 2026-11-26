use serde::Serialize;
use tauri::{AppHandle, Manager, Runtime, Webview};
use tauri_plugin_opener::OpenerExt;

use crate::server::ServerState;

/// A single row in the custom HTML menu.
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
enum MenuItemDef {
    Separator,
    Item {
        id: String,
        label: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        shortcut: Option<String>,
    },
}

/// Payload handed to the webview's menu overlay.
#[derive(Serialize)]
struct MenuPayload {
    x: f64,
    y: f64,
    items: Vec<MenuItemDef>,
}

/// Human-readable shortcut for a menu item: "Ctrl+Shift+M" on Windows and
/// Linux, "⇧⌘M" on macOS.
fn shortcut(accelerator: &str) -> String {
    #[cfg(target_os = "macos")]
    {
        accelerator
            .replace("CmdOrCtrl+Shift+", "\u{21e7}\u{2318}")
            .replace("CmdOrCtrl+", "\u{2318}")
            .replace("Shift+", "\u{21e7}")
            .replace("Alt+", "\u{2325}")
    }
    #[cfg(not(target_os = "macos"))]
    {
        accelerator.replace("CmdOrCtrl", "Ctrl")
    }
}

/// The flat item list shown in the menu. `include_edit` adds the clipboard and
/// selection items, which only the right-click context menu carries.
fn menu_items<R: Runtime>(app: &AppHandle<R>, include_edit: bool) -> Vec<MenuItemDef> {
    use MenuItemDef::{Item, Separator};
    let version = app.package_info().version.to_string();
    let mut items = Vec::new();

    if include_edit {
        items.push(Item {
            id: "copy".into(),
            label: "Copy".into(),
            shortcut: Some(shortcut("CmdOrCtrl+C")),
        });
        items.push(Item {
            id: "cut".into(),
            label: "Cut".into(),
            shortcut: Some(shortcut("CmdOrCtrl+X")),
        });
        items.push(Item {
            id: "paste".into(),
            label: "Paste".into(),
            shortcut: Some(shortcut("CmdOrCtrl+V")),
        });
        items.push(Item {
            id: "select-all".into(),
            label: "Select All".into(),
            shortcut: Some(shortcut("CmdOrCtrl+A")),
        });
        items.push(Separator);
    }

    items.push(Item {
        id: "new-window".into(),
        label: "New window".into(),
        shortcut: Some(shortcut("CmdOrCtrl+N")),
    });
    items.push(Item {
        id: "new-tab".into(),
        label: "New tab".into(),
        shortcut: Some(shortcut("CmdOrCtrl+T")),
    });
    items.push(Item {
        id: "open-stream-window".into(),
        label: "Open stream in new window".into(),
        shortcut: None,
    });
    items.push(Item {
        id: "close-tab".into(),
        label: "Close tab".into(),
        shortcut: Some(shortcut("CmdOrCtrl+W")),
    });
    items.push(Item {
        id: "close-window".into(),
        label: "Close window".into(),
        shortcut: Some(shortcut("CmdOrCtrl+Shift+W")),
    });
    items.push(Item {
        id: "toggle-mute".into(),
        label: "Mute or unmute tab".into(),
        shortcut: Some(shortcut("CmdOrCtrl+Shift+M")),
    });
    items.push(Separator);

    items.push(Item {
        id: "reload".into(),
        label: "Reload".into(),
        shortcut: Some(shortcut("CmdOrCtrl+R")),
    });
    #[cfg(debug_assertions)]
    items.push(Item {
        id: "devtools".into(),
        label: "Toggle DevTools".into(),
        shortcut: Some(shortcut("F12")),
    });
    items.push(Item {
        id: "fullscreen".into(),
        label: "Toggle Fullscreen".into(),
        shortcut: Some(shortcut("F11")),
    });
    items.push(Separator);

    items.push(Item {
        id: "twitch-login".into(),
        label: "Log in to Twitch player".into(),
        shortcut: Some(shortcut("CmdOrCtrl+Shift+T")),
    });
    items.push(Item {
        id: "twitch-help".into(),
        label: "Twitch login help".into(),
        shortcut: None,
    });
    items.push(Separator);

    items.push(Item {
        id: "restart-server".into(),
        label: "Restart server".into(),
        shortcut: None,
    });
    items.push(Item {
        id: "open-logs".into(),
        label: "Open logs".into(),
        shortcut: None,
    });
    items.push(Separator);

    items.push(Item {
        id: "check-updates".into(),
        label: "Check for updates".into(),
        shortcut: None,
    });
    items.push(Item {
        id: "about".into(),
        label: format!("About Mixchat ({version})"),
        shortcut: None,
    });
    items.push(Separator);

    items.push(Item {
        id: "quit".into(),
        label: "Quit".into(),
        shortcut: Some(shortcut("CmdOrCtrl+Q")),
    });

    items
}

/// Render the custom menu overlay in `webview` at the given viewport point.
fn open_custom_menu<R: Runtime>(
    app: &AppHandle<R>,
    webview: &Webview<R>,
    include_edit: bool,
    x: f64,
    y: f64,
) -> Result<(), String> {
    let payload = MenuPayload {
        x,
        y,
        items: menu_items(app, include_edit),
    };
    let json = serde_json::to_string(&payload).map_err(|error| error.to_string())?;
    webview
        .eval(format!(
            "window.__MIXCHAT_MENU__&&window.__MIXCHAT_MENU__.open({json});"
        ))
        .map_err(|error| error.to_string())
}

fn target_webview<R: Runtime>(app: &AppHandle<R>, target: Option<&str>) -> Option<Webview<R>> {
    target
        .and_then(|label| app.get_webview(label))
        .or_else(|| crate::tabs::first_content_webview(app))
}

pub fn handle_id<R: Runtime>(app: &AppHandle<R>, target: Option<&str>, id: &str) {
    match id {
        "quit" => app.exit(0),
        "new-window" => crate::tabs::create_window(app, None),
        "new-tab" => {
            if let Some(webview) = target_webview(app, target) {
                crate::tabs::new_tab_for_webview(app, webview.label());
            }
        }
        "open-stream-window" => {
            if let Some(webview) = target_webview(app, target) {
                crate::tabs::duplicate_in_new_window(app, webview.label());
            } else {
                crate::tabs::create_window(app, None);
            }
        }
        "close-tab" => {
            if let Some(webview) = target_webview(app, target) {
                crate::tabs::close_active_tab(app, webview.label());
            }
        }
        "close-window" => {
            if let Some(webview) = target_webview(app, target) {
                crate::tabs::close_window_for_webview(app, webview.label());
            }
        }
        "toggle-mute" => {
            if let Some(webview) = target_webview(app, target) {
                crate::tabs::toggle_active_mute(app, webview.label());
            }
        }
        "next-tab" => {
            if let Some(webview) = target_webview(app, target) {
                crate::tabs::cycle_for_webview(app, webview.label(), 1);
            }
        }
        "prev-tab" => {
            if let Some(webview) = target_webview(app, target) {
                crate::tabs::cycle_for_webview(app, webview.label(), -1);
            }
        }
        _ if id.starts_with("select-tab-") => {
            if let (Some(webview), Ok(index)) = (
                target_webview(app, target),
                id["select-tab-".len()..].parse::<usize>(),
            ) {
                crate::tabs::select_index_for_webview(app, webview.label(), index);
            }
        }
        "reload" => {
            if let Some(webview) = target_webview(app, target) {
                let _ = webview.reload();
            }
        }
        #[cfg(debug_assertions)]
        "devtools" => {
            if let Some(webview) = target_webview(app, target) {
                webview.open_devtools();
            }
        }
        "fullscreen" => {
            if let Some(webview) = target_webview(app, target) {
                let window = webview.window();
                let fullscreen = window.is_fullscreen().unwrap_or(false);
                let _ = window.set_fullscreen(!fullscreen);
            }
        }
        "restart-server" => {
            if let Some(state) = app.try_state::<std::sync::Arc<ServerState>>() {
                state.restart();
            }
        }
        "open-logs" => {
            if let Some(state) = app.try_state::<std::sync::Arc<ServerState>>() {
                let path = state.log_path().to_string_lossy().to_string();
                let _ = app.opener().open_path(path, None::<&str>);
            }
        }
        "twitch-login" => {
            if let Err(error) = crate::open_twitch_login(app) {
                eprintln!("[twitch-login] failed: {error}");
            }
        }
        "twitch-help" => {
            if let Err(error) = crate::open_twitch_help(app) {
                eprintln!("[twitch-help] failed: {error}");
            }
        }
        "check-updates" => crate::updater::check_manual(app.clone()),
        "about" => {
            if let Err(error) = crate::open_about(app) {
                eprintln!("[about] failed: {error}");
            }
        }
        _ => {}
    }
}

/// Right-click context menu: renders the custom menu (with clipboard items) in
/// the requesting content webview at the pointer.
#[tauri::command]
pub fn show_context_menu<R: Runtime>(
    app: AppHandle<R>,
    webview: Webview<R>,
    x: f64,
    y: f64,
) -> Result<(), String> {
    open_custom_menu(&app, &webview, true, x, y)
}

/// App menu opened from the tab strip's menu button (no clipboard items). It
/// renders in the workspace's active content webview just below the strip.
#[tauri::command]
pub fn show_menu<R: Runtime>(app: AppHandle<R>, webview: Webview<R>) -> Result<(), String> {
    let label = crate::tabs::active_content_label(&app, webview.label())
        .ok_or_else(|| "no active content webview".to_string())?;
    let target = app
        .get_webview(&label)
        .ok_or_else(|| "active webview is gone".to_string())?;
    let _ = target.set_focus();
    open_custom_menu(&app, &target, false, 8.0, 6.0)
}

#[tauri::command]
pub fn menu_action<R: Runtime>(app: AppHandle<R>, webview: Webview<R>, id: String) {
    handle_id(&app, Some(webview.label()), &id);
}

/// Clipboard text for the Paste menu item. Webviews refuse script-initiated
/// clipboard reads, so the shell reads the OS clipboard and returns the text to
/// the requesting webview, which inserts it at the caret. Runs off the main
/// thread because an X11 clipboard owner can take a while to answer.
#[tauri::command(async)]
pub fn read_clipboard_text() -> Result<Option<String>, String> {
    let mut clipboard = arboard::Clipboard::new().map_err(|error| error.to_string())?;
    match clipboard.get_text() {
        Ok(text) => Ok((!text.is_empty()).then_some(text)),
        // Empty clipboard or contents in a format other than text.
        Err(arboard::Error::ContentNotAvailable) => Ok(None),
        Err(error) => {
            eprintln!("[clipboard] read failed: {error}");
            Ok(None)
        }
    }
}

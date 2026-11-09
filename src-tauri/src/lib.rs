mod config;
mod logger;
mod menu;
mod server;

use std::sync::Arc;

use config::AppConfig;
use logger::Logger;
use server::ServerState;
use tauri::webview::NewWindowResponse;
use tauri::{AppHandle, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
use tauri_plugin_opener::OpenerExt;

pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .invoke_handler(tauri::generate_handler![
            menu::show_context_menu,
            menu::menu_action
        ])
        .on_menu_event(menu::handle)
        .setup(|app| {
            app.handle().set_theme(Some(tauri::Theme::Dark));
            let handle = app.handle().clone();
            let app_config = AppConfig::load(&handle)?;
            let logger = Arc::new(Logger::new(&app_config.log_dir)?);

            logger.info(format!(
                "mixchat-desktop starting mode={:?} url={}",
                app_config.mode,
                redact_url(&app_config.url)
            ));

            let state = Arc::new(ServerState::new(
                handle.clone(),
                app_config,
                logger.clone(),
            ));
            app.manage(state.clone());

            #[cfg(unix)]
            {
                let signal_handle = handle.clone();
                let signal_state = state.clone();
                tauri::async_runtime::spawn(async move {
                    use tokio::signal::unix::{signal, SignalKind};
                    let (mut terminate, mut interrupt) = match (
                        signal(SignalKind::terminate()),
                        signal(SignalKind::interrupt()),
                    ) {
                        (Ok(terminate), Ok(interrupt)) => (terminate, interrupt),
                        _ => return,
                    };
                    tokio::select! {
                        _ = terminate.recv() => {}
                        _ = interrupt.recv() => {}
                    }
                    signal_state.shutdown();
                    signal_handle.exit(0);
                });
            }

            tauri::async_runtime::spawn(async move {
                match state.start().await {
                    Ok(()) => {
                        if let Err(error) = create_main_window(&handle, &state) {
                            logger.error(format!("failed to create main window: {error}"));
                            show_error(&handle, &logger, &format!(
                                "Mixchat could not open its window.\n\n{error}"
                            ));
                        }
                    }
                    Err(error) => {
                        logger.error(format!("server failed to start: {error}"));
                        show_error(&handle, &logger, &error);
                        handle.exit(1);
                    }
                }
            });

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building the Mixchat desktop app");

    app.run(|app_handle, event| {
        if matches!(event, RunEvent::ExitRequested { .. } | RunEvent::Exit) {
            if let Some(state) = app_handle.try_state::<Arc<ServerState>>() {
                state.shutdown();
            }
        }
    });
}

fn open_twitch_child<R: tauri::Runtime>(
    app: &AppHandle<R>,
    label: &str,
    title: &str,
    url: tauri::Url,
    reload_main_on_close: bool,
) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(label) {
        let _ = window.navigate(url);
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        return Ok(());
    }

    // This runs from Tauri callbacks (navigation, new-window, menu, IPC).
    // Building a Webview2 window on the event-loop thread deadlocks on Windows,
    // so create the child on its own thread and let the loop service it.
    let app = app.clone();
    let label = label.to_string();
    let title = title.to_string();
    std::thread::spawn(move || {
        let navigation_app = app.clone();
        let popup_app = app.clone();
        let main_window = app.get_webview_window("main");
        let nav_label = label.clone();
        let popup_label = label.clone();
        #[cfg(debug_assertions)]
        eprintln!("[twitch-child] opening {label}");

        let window = match WebviewWindowBuilder::new(&app, label.as_str(), WebviewUrl::External(url))
            .title(title)
            .theme(Some(tauri::Theme::Dark))
            .inner_size(600.0, 760.0)
            .min_inner_size(400.0, 500.0)
            .on_navigation(move |target| {
                let scheme = target.scheme();
                if scheme != "http" && scheme != "https" {
                    return false;
                }
                if target.host_str() == Some("www.twitch.tv") && target.path() == "/" {
                    if let Some(window) = navigation_app.get_webview_window(&nav_label) {
                        let _ = window.close();
                    }
                }
                true
            })
            .on_new_window(move |target, _features| {
                // Keep Twitch login popups (SSO, verification) in the child
                // window so they share the same cookie jar.
                if let Some(window) = popup_app.get_webview_window(&popup_label) {
                    let _ = window.navigate(target);
                }
                NewWindowResponse::Deny
            })
            .build()
        {
            Ok(window) => window,
            Err(error) => {
                eprintln!("[twitch-child] failed to open {label}: {error}");
                return;
            }
        };
        let _ = window.set_theme(Some(tauri::Theme::Dark));

        // The player only sees the new cookies after its iframe is recreated.
        if reload_main_on_close {
            if let Some(main) = main_window {
                let main = main.clone();
                window.on_window_event(move |event| {
                    if matches!(event, tauri::WindowEvent::Destroyed) {
                        let _ = main.reload();
                    }
                });
            }
        }
    });

    Ok(())
}

/// Opens Twitch's own login page in a child webview. The embedded player reads
/// Twitch's session cookies, not the API access token, so this is what makes
/// the player show the signed-in account.
pub(crate) fn open_twitch_login<R: tauri::Runtime>(
    app: &AppHandle<R>,
) -> tauri::Result<()> {
    let url: tauri::Url = "https://www.twitch.tv/login".parse().unwrap();
    open_twitch_child(app, "twitch-login", "Log in to Twitch", url, true)
}

/// Opens the Device Code activation page in a child webview. The same login
/// also stores Twitch cookies in the shared WebView2 profile, so the embedded
/// player is signed in without a second login.
pub(crate) fn open_twitch_activation<R: tauri::Runtime>(
    app: &AppHandle<R>,
    url: tauri::Url,
) -> tauri::Result<()> {
    open_twitch_child(app, "twitch-activate", "Authorize Twitch", url, false)
}

/// Small About window with project links. Content is written to a temp HTML
/// file and loaded over `file://` so the page needs no IPC or custom protocol.
pub(crate) fn open_about<R: tauri::Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let version = app.package_info().version.to_string();
    let path = std::env::temp_dir().join("mixchat-about.html");
    std::fs::write(&path, about_html(&version))?;
    let url = tauri::Url::from_file_path(&path)
        .map_err(|_| tauri::Error::InvalidWebviewUrl("about page path"))?;

    if let Some(window) = app.get_webview_window("about") {
        let _ = window.navigate(url);
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        return Ok(());
    }

    // Same event-loop constraint as the Twitch child windows: build off-thread.
    let app = app.clone();
    std::thread::spawn(move || {
        let navigation_app = app.clone();
        let window = WebviewWindowBuilder::new(&app, "about", WebviewUrl::External(url))
            .title(format!("About Mixchat {version}"))
            .theme(Some(tauri::Theme::Dark))
            .inner_size(460.0, 420.0)
            .resizable(false)
            .maximizable(false)
            .center()
            .skip_taskbar(true)
            .on_navigation(move |target| {
                if matches!(target.scheme(), "http" | "https") {
                    open_external_url(&navigation_app, target);
                    return false;
                }
                true
            })
            .on_new_window(|_, _| NewWindowResponse::Deny)
            .build();
        match window {
            Ok(window) => {
                let _ = window.set_theme(Some(tauri::Theme::Dark));
                let _ = window.set_focus();
            }
            Err(error) => eprintln!("[about] failed to open: {error}"),
        }
    });

    Ok(())
}

fn about_html(version: &str) -> String {
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8" />
<meta name="viewport" content="width=device-width, initial-scale=1" />
<title>About Mixchat</title>
<style>
  :root {{ color-scheme: dark; }}
  * {{ box-sizing: border-box; }}
  body {{
    margin: 0;
    padding: 28px 28px 24px;
    font-family: system-ui, -apple-system, "Segoe UI", sans-serif;
    background: #0a0a0a;
    color: #e5e5e5;
    line-height: 1.5;
  }}
  h1 {{
    margin: 0;
    font-size: 1.35rem;
    font-weight: 650;
    letter-spacing: -0.02em;
  }}
  .version {{
    margin: 4px 0 0;
    color: #a3a3a3;
    font-size: 0.95rem;
  }}
  .desc {{
    margin: 16px 0 20px;
    color: #a3a3a3;
    font-size: 0.9rem;
  }}
  ul {{
    list-style: none;
    margin: 0;
    padding: 0;
  }}
  li {{ margin: 0; }}
  a {{
    display: block;
    padding: 9px 12px;
    margin: 0 -12px;
    border-radius: 8px;
    color: #7dd3fc;
    text-decoration: none;
    font-size: 0.95rem;
  }}
  a:hover {{ background: #171717; text-decoration: underline; }}
  .meta {{
    margin-top: 22px;
    padding-top: 14px;
    border-top: 1px solid #262626;
    color: #737373;
    font-size: 0.8rem;
  }}
</style>
</head>
<body>
  <h1>Mixchat Desktop</h1>
  <p class="version">Version {version}</p>
  <p class="desc">Tauri desktop shell that runs the Mixchat server locally.</p>
  <nav aria-label="Project links">
    <ul>
      <li><a href="https://github.com/grepfs17/MixchatDesktop">GitHub repository</a></li>
      <li><a href="https://github.com/grepfs17/MixchatDesktop/issues">Report an issue</a></li>
      <li><a href="https://github.com/grepfs17/Mixchat">Mixchat source</a></li>
      <li><a href="https://mixchat.grepfs.xyz/">mixchat.grepfs.xyz</a></li>
    </ul>
  </nav>
  <p class="meta">Licensed under AGPL-3.0 · © 2026 Mixchat</p>
</body>
</html>
"#
    )
}

/// Hand a non-app URL to the system handler. Only `http`/`https`/`mailto`
/// are allowed: the opener plugin's Rust-side methods do not enforce the
/// plugin URL scope, so page-driven navigation must be filtered here or it
/// could trigger `smb://`, `file://`, or other custom schemes.
fn open_external_url<R: tauri::Runtime>(app: &AppHandle<R>, url: &tauri::Url) {
    if matches!(url.scheme(), "http" | "https" | "mailto") {
        let _ = app.opener().open_url(url.as_str(), None::<&str>);
    } else {
        eprintln!("[nav] blocked non-http(s)/mailto URL: {}", url.scheme());
    }
}

fn twitch_activation_target(target: &tauri::Url) -> Option<tauri::Url> {
    if target.path() != "/desktop/twitch-activate" {
        return None;
    }
    let raw = target
        .query_pairs()
        .find(|(key, _)| key.as_ref() == "url")?
        .1
        .to_string();
    let parsed: tauri::Url = raw.parse().ok()?;
    if parsed.scheme() != "https" {
        return None;
    }
    let host = parsed.host_str()?;
    if host != "www.twitch.tv" && host != "twitch.tv" {
        return None;
    }
    if parsed.path() != "/activate" {
        return None;
    }
    Some(parsed)
}

fn create_main_window(app: &AppHandle, state: &Arc<ServerState>) -> tauri::Result<()> {
    if app.get_webview_window("main").is_some() {
        return Ok(());
    }

    let mut url: tauri::Url = state
        .url()
        .parse()
        .unwrap_or_else(|_| "http://localhost:3000".parse().unwrap());
    // The query parameter is the reliable shell marker. It survives the
    // webview initialization script being skipped on remote URLs.
    url.query_pairs_mut()
        .append_pair("mixchat-shell", "tauri");
    #[cfg(debug_assertions)]
    eprintln!("[window] loading {url}");

    let navigation_app = app.clone();
    let new_window_app = app.clone();
    // Exact origin (scheme + host + port), matching the old Electron
    // allowlist. Host-only comparison would also admit http:// when the app
    // is on https://, and any-port loopback would admit unrelated local
    // services (port scanners, printers, dev servers).
    let app_origin = url.origin().ascii_serialization();
    let new_window_origin = app_origin.clone();

    let window = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url))
        .title("Mixchat")
        .theme(Some(tauri::Theme::Dark))
        .inner_size(1600.0, 900.0)
        .min_inner_size(900.0, 600.0)
        .initialization_script(
            r#"window.__MIXCHAT_SHELL__ = 'tauri';
window.addEventListener('contextmenu', (event) => {
  event.preventDefault();
  window.__TAURI_INTERNALS__.invoke('show_context_menu');
});
window.addEventListener('keydown', (event) => {
  const mod = event.ctrlKey || event.metaKey;
  const key = event.key.toLowerCase();
  if (mod && event.shiftKey && key === 't') {
    event.preventDefault();
    window.__TAURI_INTERNALS__.invoke('menu_action', { id: 'twitch-login' });
  } else if (mod && !event.altKey && key === 'r') {
    event.preventDefault();
    window.__TAURI_INTERNALS__.invoke('menu_action', { id: 'reload' });
  } else if (mod && !event.altKey && key === 'q') {
    event.preventDefault();
    window.__TAURI_INTERNALS__.invoke('menu_action', { id: 'quit' });
  } else if (!mod && event.key === 'F11') {
    event.preventDefault();
    window.__TAURI_INTERNALS__.invoke('menu_action', { id: 'fullscreen' });
  } else if (!mod && event.key === 'F12') {
    event.preventDefault();
    window.__TAURI_INTERNALS__.invoke('menu_action', { id: 'devtools' });
  }
});"#,
        )
        .on_navigation(move |target| {
            let is_app_origin = target.origin().ascii_serialization() == app_origin;
            if is_app_origin {
                if target.path() == "/desktop/twitch-login" {
                    let _ = open_twitch_login(&navigation_app);
                    return false;
                }
                if let Some(activation) = twitch_activation_target(target) {
                    let _ = open_twitch_activation(&navigation_app, activation);
                    return false;
                }
                return true;
            }
            open_external_url(&navigation_app, target);
            false
        })
        .on_new_window(move |target, _features| {
            let is_app_origin = target.origin().ascii_serialization() == new_window_origin;
            let is_twitch_login =
                is_app_origin && target.path() == "/desktop/twitch-login";
            if is_twitch_login {
                let _ = open_twitch_login(&new_window_app);
                return NewWindowResponse::Deny;
            }
            if is_app_origin {
                if let Some(activation) = twitch_activation_target(&target) {
                    let _ = open_twitch_activation(&new_window_app, activation);
                    return NewWindowResponse::Deny;
                }
            }
            open_external_url(&new_window_app, &target);
            NewWindowResponse::Deny
        })
        .build()?;
    let _ = window.set_theme(Some(tauri::Theme::Dark));

    let _ = window.set_focus();
    Ok(())
}

/// Strip `user:password@` from a URL before it is logged.
fn redact_url(url: &str) -> String {
    match url.parse::<tauri::Url>() {
        Ok(mut parsed) => {
            if !parsed.username().is_empty() || parsed.password().is_some() {
                let _ = parsed.set_username("");
                let _ = parsed.set_password(None);
            }
            parsed.to_string()
        }
        Err(_) => url.to_string(),
    }
}

fn show_error(app: &AppHandle, logger: &Logger, message: &str) {
    app.dialog()
        .message(message)
        .kind(MessageDialogKind::Error)
        .title("Mixchat failed to start")
        .blocking_show();
    logger.error(format!("logs: {}", logger.path().display()));
}

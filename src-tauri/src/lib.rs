mod config;
mod downloads;
mod logger;
mod loopback;
mod menu;
mod server;
mod tabs;
mod updater;

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
            tabs::focus_first_window(app);
        }))
        .plugin(tauri_plugin_dialog::init())
        // The webview has no IPC permissions, so the plugin's click interceptor
        // must stay off. It prevents the default action and then fails to open
        // the URL. Rust-side navigation handling opens external links instead.
        // tauri-plugin-shell does the same interception with JS it always
        // injects, and it has no switch to turn that off, so it is not used.
        .plugin(
            tauri_plugin_opener::Builder::new()
                .open_js_links_on_click(false)
                .build(),
        )
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .register_uri_scheme_protocol(tabs::SCHEME, |_ctx, request| {
            tabs::tab_strip_response(&request)
        })
        .invoke_handler(tauri::generate_handler![
            menu::show_context_menu,
            menu::menu_action,
            downloads::download_start,
            downloads::download_snapshot,
            downloads::download_cancel,
            downloads::download_dismiss,
            downloads::download_reveal,
            tabs::tabs_ready,
            tabs::tabs_select,
            tabs::tabs_close,
            tabs::tabs_new,
            tabs::tabs_new_window,
            tabs::tabs_toggle_mute,
            tabs::tabs_cycle,
            tabs::tabs_select_index,
            tabs::tab_meta
        ])
        .on_menu_event(menu::handle)
        .setup(|app| {
            app.handle().set_theme(Some(tauri::Theme::Dark));
            let handle = app.handle().clone();
            let app_config = AppConfig::load(&handle)?;
            downloads::init_tls();
            app.manage(downloads::Downloads::new(&app_config.url));
            app.manage(tabs::Tabs::new());
            app.manage(menu::ContextTarget::default());
            let auto_update =
                app_config.mode == config::Mode::Packaged && updater::updates_supported();
            let logger = Arc::new(Logger::new(&app_config.log_dir)?);

            logger.info(format!(
                "mixchat-desktop starting mode={:?} url={}",
                app_config.mode,
                redact_url(&app_config.url)
            ));

            let state = Arc::new(ServerState::new(app_config, logger.clone()));
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
                        tabs::create_window(&handle, None);
                        if auto_update {
                            updater::check_in_background(handle.clone());
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

        // The player only sees the new cookies after its iframe is recreated,
        // and every tab may be showing a stream.
        if reload_main_on_close {
            let reload_app = app.clone();
            window.on_window_event(move |event| {
                if matches!(event, tauri::WindowEvent::Destroyed) {
                    tabs::reload_all(&reload_app);
                }
            });
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

/// Opens the Twitch login guide in a child webview. The shell opens it when
/// the device login starts and closes it through `/desktop/twitch-close` when
/// the login succeeds or is cancelled.
pub(crate) fn open_twitch_help<R: tauri::Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let base = app
        .try_state::<Arc<ServerState>>()
        .map(|state| state.url().to_string())
        .unwrap_or_else(|| "http://localhost:3000".to_string());
    let mut url: tauri::Url = format!("{}/help/twitch-login", base.trim_end_matches('/'))
        .parse()
        .map_err(|_| tauri::Error::InvalidWebviewUrl("help url"))?;
    url.query_pairs_mut().append_pair("mixchat-shell", "tauri");
    open_twitch_child(app, "twitch-help", "Twitch login help", url, false)
}

/// Closes the Twitch login child windows (guide and activation page) when the
/// page signals that the device login succeeded or was cancelled.
pub(crate) fn close_twitch_login_windows<R: tauri::Runtime>(app: &AppHandle<R>) {
    for label in ["twitch-help", "twitch-activate"] {
        if let Some(window) = app.get_webview_window(label) {
            let _ = window.close();
        }
    }
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
pub(crate) fn open_external_url<R: tauri::Runtime>(app: &AppHandle<R>, url: &tauri::Url) {
    if matches!(url.scheme(), "http" | "https" | "mailto") {
        let _ = app.opener().open_url(url.as_str(), None::<&str>);
    } else {
        eprintln!("[nav] blocked non-http(s)/mailto URL: {}", url.scheme());
    }
}

pub(crate) fn twitch_activation_target(target: &tauri::Url) -> Option<tauri::Url> {
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

use std::collections::HashMap;
use std::error::Error;
use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Dev,
    Local,
    Packaged,
}

impl Mode {
    fn resolve(value: Option<String>) -> Self {
        match value.as_deref() {
            Some("dev") => Mode::Dev,
            Some("local") => Mode::Local,
            Some("packaged") => Mode::Packaged,
            _ if cfg!(debug_assertions) => Mode::Dev,
            _ => Mode::Packaged,
        }
    }
}

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub mode: Mode,
    pub host: String,
    pub port: u16,
    pub url: String,
    pub data_dir: PathBuf,
    pub log_dir: PathBuf,
    pub server_entry: Option<PathBuf>,
    /// Environment values the server must receive. The child inherits the
    /// process environment, so this only contains file values and overrides.
    pub env: HashMap<String, String>,
}

impl AppConfig {
    pub fn load(app: &AppHandle) -> Result<Self, Box<dyn Error>> {
        // Portable builds override the app directories with a relative path
        // such as ./app-data. Anchor it to the executable so the server cwd
        // does not depend on where the app was launched from.
        let data_dir = exe_anchored(app.path().app_data_dir()?);
        let log_dir = exe_anchored(app.path().app_log_dir()?);
        std::fs::create_dir_all(&data_dir)?;
        std::fs::create_dir_all(&log_dir)?;

        // `.env` search order. The repository (CARGO_MANIFEST_DIR) and cwd
        // candidates are debug-only: a release binary must not read a planted
        // `.env` from the build machine's source tree or the launch directory
        // (classic hijack vector next to a signed executable).
        let mut env_candidates: Vec<Option<PathBuf>> = vec![
            std::env::var_os("MIXCHAT_CONFIG").map(PathBuf::from),
        ];
        #[cfg(debug_assertions)]
        {
            env_candidates.push(
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .parent()
                    .map(Path::to_path_buf)
                    .map(|root| root.join(".env")),
            );
        }
        env_candidates.extend([
            Some(data_dir.join(".env")),
            app.path()
                .app_config_dir()
                .ok()
                .map(|p| exe_anchored(p).join(".env")),
            std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|d| d.join(".env"))),
        ]);
        #[cfg(debug_assertions)]
        {
            env_candidates.push(std::env::current_dir().ok().map(|p| p.join(".env")));
        }
        let file_env = load_first_env_file(&env_candidates);

        let setting = |key: &str| {
            std::env::var(key)
                .ok()
                .or_else(|| file_env.get(key).cloned())
        };

        let mode = Mode::resolve(setting("MIXCHAT_MODE"));
        let mut host = setting("MIXCHAT_HOST").unwrap_or_else(|| "127.0.0.1".to_string());
        let port = setting("MIXCHAT_PORT")
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(3000);
        let url = setting("MIXCHAT_URL")
            .unwrap_or_else(|| format!("http://localhost:{port}"));

        // Packaged builds must not expose the local server to the LAN by
        // accident: privileged endpoints (/api/desktop/open, auth handoff)
        // assume a loopback-only peer.
        if mode == Mode::Packaged && !is_loopback_host(&host) {
            eprintln!(
                "[mixchat] MIXCHAT_HOST={host} is not loopback; forcing 127.0.0.1 in packaged mode"
            );
            host = "127.0.0.1".to_string();
        }

        let server_entry = match mode {
            Mode::Dev => None,
            Mode::Local => {
                // Local mode only runs from a source checkout (pnpm start /
                // tauri dev). Resolve the repo root without embedding the
                // build machine's absolute path in release binaries.
                #[cfg(debug_assertions)]
                let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| PathBuf::from("."));
                #[cfg(not(debug_assertions))]
                let project_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                Some(
                    project_root
                        .join("build")
                        .join("mixchat")
                        .join("dist")
                        .join("server")
                        .join("entry.mjs"),
                )
            }
            Mode::Packaged => {
                // Portable builds keep resources next to the executable,
                // while Linux packages install them under /usr/lib. Prefer the
                // adjacent copy when it exists.
                let adjacent = std::env::current_exe()
                    .ok()
                    .and_then(|exe| exe.parent().map(Path::to_path_buf))
                    .map(|dir| {
                        dir.join("mixchat")
                            .join("dist")
                            .join("server")
                            .join("entry.mjs")
                    })
                    .filter(|path| path.is_file());

                Some(match adjacent {
                    Some(path) => path,
                    None => app
                        .path()
                        .resource_dir()?
                        .join("mixchat")
                        .join("dist")
                        .join("server")
                        .join("entry.mjs"),
                })
            }
        };

        // File values fill gaps. The child inherits process env, so anything
        // already exported must not be overwritten here.
        //
        // Only allowlisted keys from `.env` files are forwarded: a planted
        // file could otherwise set NODE_OPTIONS / LD_PRELOAD / OPENSSL_CONF
        // and execute code inside the sidecar under a clean binary.
        let mut env: HashMap<String, String> = file_env
            .into_iter()
            .filter(|(key, value)| {
                std::env::var_os(key).is_none() && is_sidecar_env_key(key) && !value.is_empty()
            })
            .collect();

        env.insert("PORT".to_string(), port.to_string());
        env.insert("HOST".to_string(), host.clone());
        env.insert("NODE_ENV".to_string(), "production".to_string());

        if !env.contains_key("ORIGIN") && std::env::var_os("ORIGIN").is_none() {
            env.insert("ORIGIN".to_string(), format!("http://localhost:{port}"));
        }

        let song_queue = data_dir.join("song-queue");
        let wheel = data_dir.join("wheel");
        let stream_tools = data_dir.join("stream-tools");
        std::fs::create_dir_all(&song_queue)?;
        std::fs::create_dir_all(&wheel)?;
        std::fs::create_dir_all(&stream_tools)?;
        env.insert(
            "MIXCHAT_SONG_QUEUE_DIR".to_string(),
            song_queue.to_string_lossy().to_string(),
        );
        env.insert(
            "MIXCHAT_WHEEL_DIR".to_string(),
            wheel.to_string_lossy().to_string(),
        );
        env.insert(
            "MIXCHAT_STREAM_TOOLS_DIR".to_string(),
            stream_tools.to_string_lossy().to_string(),
        );

        Ok(Self {
            mode,
            host,
            port,
            url,
            data_dir,
            log_dir,
            server_entry,
            env,
        })
    }
}

fn exe_anchored(path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        return path;
    }
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(&path)))
        .unwrap_or(path)
}

fn is_loopback_host(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.parse::<std::net::IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
}

/// Keys from `.env` files that may be forwarded to the Node sidecar.
///
/// The child also inherits the process environment; this allowlist only
/// gates values loaded from disk so a planted `.env` cannot inject
/// interpreters (`NODE_OPTIONS`, `LD_PRELOAD`, `OPENSSL_CONF`, …).
fn is_sidecar_env_key(key: &str) -> bool {
    const DENIED_PREFIXES: &[&str] = &[
        "NODE_",
        "LD_",
        "DYLD_",
        "OPENSSL_",
        "PYTHON",
        "PERL5",
        "RUBYOPT",
        "BASH_ENV",
        "GIT_",
        "SSH_",
        "SSLKEYLOGFILE",
    ];
    if key != "NODE_ENV" && DENIED_PREFIXES.iter().any(|p| key.starts_with(p)) {
        return false;
    }

    const ALLOWED: &[&str] = &[
        "ORIGIN",
        "PORT",
        "HOST",
        "NODE_ENV",
        "PUBLIC_APP_PLATFORM",
        "PUBLIC_TWITCH_CLIENT_ID",
        "PUBLIC_TWITCH_ONLY",
        "PUBLIC_YOUTUBE_CLIENT_ID",
        "YOUTUBE_CLIENT_SECRET",
        "ASTRO_NODE_AUTOSTART",
        "ASTRO_NODE_LOGGING",
        "DEBUG",
        "TRACE_DEPRECATION",
        "NO_DEPRECATION",
        "SERVER_CERT_PATH",
        "SERVER_KEY_PATH",
        "MIXCHAT_SONG_QUEUE_DIR",
        "MIXCHAT_WHEEL_DIR",
        "MIXCHAT_STREAM_TOOLS_DIR",
    ];
    if ALLOWED.contains(&key) {
        return true;
    }
    // YouTube InnerTube tuning and other shell-owned runtime knobs.
    key.starts_with("YT_")
}

fn load_first_env_file(candidates: &[Option<PathBuf>]) -> HashMap<String, String> {
    for candidate in candidates.iter().flatten() {
        if !candidate.is_file() {
            continue;
        }
        let iter = match dotenvy::from_path_iter(candidate) {
            Ok(iter) => iter,
            Err(_) => continue,
        };
        return iter.filter_map(Result::ok).collect();
    }
    HashMap::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_accepts_server_keys() {
        assert!(is_sidecar_env_key("ORIGIN"));
        assert!(is_sidecar_env_key("YT_DEBUG"));
        assert!(is_sidecar_env_key("YT_POTOKEN"));
        assert!(is_sidecar_env_key("NODE_ENV"));
        assert!(is_sidecar_env_key("MIXCHAT_SONG_QUEUE_DIR"));
        assert!(is_sidecar_env_key("PUBLIC_YOUTUBE_CLIENT_ID"));
        assert!(is_sidecar_env_key("YOUTUBE_CLIENT_SECRET"));
    }

    #[test]
    fn allowlist_rejects_code_execution_keys() {
        assert!(!is_sidecar_env_key("NODE_OPTIONS"));
        assert!(!is_sidecar_env_key("NODE_REPL_EXTERNAL_MODULE"));
        assert!(!is_sidecar_env_key("LD_PRELOAD"));
        assert!(!is_sidecar_env_key("LD_LIBRARY_PATH"));
        assert!(!is_sidecar_env_key("DYLD_INSERT_LIBRARIES"));
        assert!(!is_sidecar_env_key("OPENSSL_CONF"));
        assert!(!is_sidecar_env_key("BASH_ENV"));
        assert!(!is_sidecar_env_key("PYTHONSTARTUP"));
        assert!(!is_sidecar_env_key("SSLKEYLOGFILE"));
        assert!(!is_sidecar_env_key("RANDOM_EVIL_KEY"));
    }

    #[test]
    fn loopback_detection() {
        assert!(is_loopback_host("127.0.0.1"));
        assert!(is_loopback_host("localhost"));
        assert!(is_loopback_host("::1"));
        assert!(is_loopback_host("127.5.5.5"));
        assert!(!is_loopback_host("0.0.0.0"));
        assert!(!is_loopback_host("192.168.1.10"));
        assert!(!is_loopback_host("example.com"));
    }

    /// The portable override must keep the WebView2 profile (login cookies)
    /// on the OS default path so sign-in survives re-extracting the zip.
    #[test]
    fn portable_override_keeps_webview_data_stable() {
        let raw = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tauri.portable.conf.json"),
        )
        .expect("read tauri.portable.conf.json");
        let value: serde_json::Value = serde_json::from_str(&raw).expect("parse portable config");
        let override_value = value
            .get("app")
            .and_then(|app| app.get("appDirectoriesOverride"))
            .cloned()
            .expect("appDirectoriesOverride present");
        let parsed: tauri::utils::config::AppDirectoriesOverride =
            serde_json::from_value(override_value).expect("valid AppDirectoriesOverride");
        match parsed {
            tauri::utils::config::AppDirectoriesOverride::Directories(dirs) => {
                assert!(
                    dirs.local_data.is_none(),
                    "localData must stay unset so the webview profile uses %LOCALAPPDATA%"
                );
                assert_eq!(dirs.config.as_deref(), Some(Path::new("./app-data")));
                assert_eq!(dirs.data.as_deref(), Some(Path::new("./app-data")));
                assert_eq!(dirs.cache.as_deref(), Some(Path::new("./app-data/caches")));
                assert_eq!(dirs.log.as_deref(), Some(Path::new("./app-data/logs")));
            }
            other => panic!("expected Directories override, got {other:?}"),
        }
    }
}

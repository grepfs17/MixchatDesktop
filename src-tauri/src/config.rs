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
        let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));

        // Portable builds override the app directories with a relative path
        // such as ./app-data. Anchor it to the executable so the server cwd
        // does not depend on where the app was launched from.
        let data_dir = exe_anchored(app.path().app_data_dir()?);
        let log_dir = exe_anchored(app.path().app_log_dir()?);
        std::fs::create_dir_all(&data_dir)?;
        std::fs::create_dir_all(&log_dir)?;

        let file_env = load_first_env_file(&[
            std::env::var_os("MIXCHAT_CONFIG").map(PathBuf::from),
            Some(project_root.join(".env")),
            Some(data_dir.join(".env")),
            app.path()
                .app_config_dir()
                .ok()
                .map(|p| exe_anchored(p).join(".env")),
            std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|d| d.join(".env"))),
            std::env::current_dir().ok().map(|p| p.join(".env")),
        ]);

        let setting = |key: &str| {
            std::env::var(key)
                .ok()
                .or_else(|| file_env.get(key).cloned())
        };

        let mode = Mode::resolve(setting("MIXCHAT_MODE"));
        let host = setting("MIXCHAT_HOST").unwrap_or_else(|| "127.0.0.1".to_string());
        let port = setting("MIXCHAT_PORT")
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(3000);
        let url = setting("MIXCHAT_URL")
            .unwrap_or_else(|| format!("http://localhost:{port}"));

        let server_entry = match mode {
            Mode::Dev => None,
            Mode::Local => Some(
                project_root
                    .join("build")
                    .join("mixchat")
                    .join("dist")
                    .join("server")
                    .join("entry.mjs"),
            ),
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
        let mut env: HashMap<String, String> = file_env
            .into_iter()
            .filter(|(key, _)| std::env::var_os(key).is_none())
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

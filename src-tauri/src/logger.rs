use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;

pub struct Logger {
    file: Mutex<Option<File>>,
    path: PathBuf,
}

impl Logger {
    pub fn new(log_dir: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(log_dir)?;
        let path = log_dir.join("mixchat-server.log");

        if let Ok(meta) = std::fs::metadata(&path) {
            if meta.len() > MAX_LOG_BYTES {
                let _ = std::fs::rename(&path, log_dir.join("mixchat-server.log.1"));
            }
        }

        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self {
            file: Mutex::new(Some(file)),
            path,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn info(&self, message: impl AsRef<str>) {
        self.write("INFO", message.as_ref());
    }

    pub fn warn(&self, message: impl AsRef<str>) {
        self.write("WARN", message.as_ref());
    }

    pub fn error(&self, message: impl AsRef<str>) {
        self.write("ERROR", message.as_ref());
    }

    pub fn server(&self, stream: &str, message: &str) {
        self.write("SERVER", &format!("[{stream}] {message}"));
    }

    fn write(&self, level: &str, message: &str) {
        let line = format!(
            "[{}] [{level}] {message}",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
        );
        eprintln!("{line}");
        if let Ok(mut guard) = self.file.lock() {
            if let Some(file) = guard.as_mut() {
                let _ = writeln!(file, "{line}");
                let _ = file.flush();
            }
        }
    }
}

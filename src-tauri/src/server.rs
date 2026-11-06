use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::AppHandle;
use tauri_plugin_shell::process::{CommandChild, CommandEvent};
use tauri_plugin_shell::ShellExt;

use crate::config::{AppConfig, Mode};
use crate::logger::Logger;

pub struct ServerState {
    app: AppHandle,
    config: AppConfig,
    logger: Arc<Logger>,
    child: Mutex<Option<(u64, CommandChild)>>,
    shutting_down: AtomicBool,
    restarting: AtomicBool,
    attempts: AtomicU32,
    generation: AtomicU64,
}

impl ServerState {
    pub fn new(app: AppHandle, config: AppConfig, logger: Arc<Logger>) -> Self {
        Self {
            app,
            config,
            logger,
            child: Mutex::new(None),
            shutting_down: AtomicBool::new(false),
            restarting: AtomicBool::new(false),
            attempts: AtomicU32::new(0),
            generation: AtomicU64::new(0),
        }
    }

    pub fn url(&self) -> &str {
        &self.config.url
    }

    pub fn log_path(&self) -> std::path::PathBuf {
        self.logger.path().to_path_buf()
    }

    pub fn restart(self: &Arc<Self>) {
        self.logger.info("restart requested");
        self.generation.fetch_add(1, Ordering::SeqCst);
        if let Some((_, child)) = self.child.lock().unwrap().take() {
            let _ = child.kill();
        }
        if self.config.mode == Mode::Dev {
            self.logger.warn("restart ignored in dev mode");
            return;
        }
        self.attempts.store(0, Ordering::SeqCst);
        if let Err(error) = self.spawn() {
            self.logger.error(format!("server restart failed: {error}"));
        }
    }

    pub async fn start(self: &Arc<Self>) -> Result<(), String> {
        if self.config.mode == Mode::Dev {
            self.logger.info(format!(
                "dev mode: waiting for external server at {}",
                self.config.url
            ));
            return self.wait_for_health(Duration::from_secs(60)).await;
        }

        if port_in_use(&self.config.host, self.config.port) {
            return Err(format!(
                "Port {} is already in use. Close the other Mixchat instance or set MIXCHAT_PORT.",
                self.config.port
            ));
        }

        self.spawn()?;
        self.wait_for_health(Duration::from_secs(60)).await
    }

    fn spawn(self: &Arc<Self>) -> Result<(), String> {
        if self.shutting_down.load(Ordering::SeqCst) {
            return Err("app is shutting down".to_string());
        }
        if self.child.lock().unwrap().is_some() {
            return Err("server is already running".to_string());
        }

        let entry = self
            .config
            .server_entry
            .as_ref()
            .ok_or_else(|| "server entry is not configured".to_string())?;

        if !entry.is_file() {
            return Err(format!(
                "server build not found at {}. Run pnpm tauri:prepare first.",
                entry.display()
            ));
        }

        let mut command = self
            .app
            .shell()
            .sidecar("mixchat-node")
            .map_err(|error| error.to_string())?
            .arg(entry)
            .current_dir(&self.config.data_dir)
            .set_process_group(true)
            .kill_on_drop(true);

        for (key, value) in &self.config.env {
            command = command.env(key, value);
        }

        let (mut rx, child) = command.spawn().map_err(|error| error.to_string())?;
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;

        self.logger.info(format!(
            "server started pid={} entry={}",
            child.pid(),
            entry.display()
        ));
        *self.child.lock().unwrap() = Some((generation, child));

        let state = self.clone();
        tauri::async_runtime::spawn(async move {
            while let Some(event) = rx.recv().await {
                match event {
                    CommandEvent::Stdout(bytes) => {
                        // The server prints its listening line on startup, so
                        // output means the restart counter can be cleared.
                        state.attempts.store(0, Ordering::SeqCst);
                        state
                            .logger
                            .server("stdout", &String::from_utf8_lossy(&bytes));
                    }
                    CommandEvent::Stderr(bytes) => {
                        state
                            .logger
                            .server("stderr", &String::from_utf8_lossy(&bytes));
                    }
                    CommandEvent::Error(error) => {
                        state.logger.error(format!("server process error: {error}"));
                    }
                    CommandEvent::Terminated(payload) => {
                        state.logger.error(format!(
                            "server exited code={:?} signal={:?}",
                            payload.code, payload.signal
                        ));
                        {
                            let mut child = state.child.lock().unwrap();
                            if child.as_ref().map(|(current, _)| *current) == Some(generation) {
                                *child = None;
                            }
                        }
                        if !state.shutting_down.load(Ordering::SeqCst)
                            && state.generation.load(Ordering::SeqCst) == generation
                        {
                            state.schedule_restart();
                        }
                        break;
                    }
                    _ => {}
                }
            }
        });

        Ok(())
    }

    async fn wait_for_health(&self, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.shutting_down.load(Ordering::SeqCst) {
                return Err("app is shutting down".to_string());
            }
            if http_ok(&self.config.host, self.config.port) {
                self.logger.info(format!(
                    "server ready at http://{}:{}",
                    self.config.host, self.config.port
                ));
                self.attempts.store(0, Ordering::SeqCst);
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        Err(format!(
            "timed out waiting for http://{}:{}",
            self.config.host, self.config.port
        ))
    }

    fn schedule_restart(self: &Arc<Self>) {
        if self.restarting.swap(true, Ordering::SeqCst) {
            return;
        }
        let state = self.clone();
        tauri::async_runtime::spawn(async move {
            let attempt = state.attempts.fetch_add(1, Ordering::SeqCst) + 1;
            if attempt > 5 {
                state.logger.error("server crashed too many times, giving up");
                state.restarting.store(false, Ordering::SeqCst);
                return;
            }

            let delay_secs = std::cmp::min(30, 1u64 << (attempt - 1));
            state.logger.warn(format!(
                "restarting server in {delay_secs}s (attempt {attempt})"
            ));
            tokio::time::sleep(Duration::from_secs(delay_secs)).await;

            if state.shutting_down.load(Ordering::SeqCst) {
                state.restarting.store(false, Ordering::SeqCst);
                return;
            }

            if let Err(error) = state.spawn() {
                state.logger.error(format!("server restart failed: {error}"));
            }
            state.restarting.store(false, Ordering::SeqCst);
        });
    }

    pub fn shutdown(&self) {
        self.shutting_down.store(true, Ordering::SeqCst);
        if let Some((_, child)) = self.child.lock().unwrap().take() {
            if let Err(error) = child.kill() {
                self.logger.warn(format!("failed to kill server: {error}"));
            }
        }
    }
}

fn health_host(host: &str) -> &str {
    match host {
        "0.0.0.0" => "127.0.0.1",
        "::" => "::1",
        other => other,
    }
}

fn resolve_addr(host: &str, port: u16) -> Option<SocketAddr> {
    (health_host(host), port)
        .to_socket_addrs()
        .ok()
        .and_then(|mut addresses| addresses.next())
}

fn port_in_use(host: &str, port: u16) -> bool {
    let Some(address) = resolve_addr(host, port) else {
        return false;
    };
    TcpStream::connect_timeout(&address, Duration::from_millis(300)).is_ok()
}

fn http_ok(host: &str, port: u16) -> bool {
    let host = health_host(host);
    let Some(address) = resolve_addr(host, port) else {
        return false;
    };
    let Ok(mut stream) = TcpStream::connect_timeout(&address, Duration::from_millis(400)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(800)));
    let request = format!(
        "GET /api/parent-domain HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
    );
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }
    let mut buffer = [0u8; 64];
    match stream.read(&mut buffer) {
        Ok(read) if read > 0 => {
            let status = &buffer[..read];
            status.starts_with(b"HTTP/1.1 200") || status.starts_with(b"HTTP/1.0 200")
        }
        _ => false,
    }
}

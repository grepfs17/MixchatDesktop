use std::io;
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// Owns the loopback addresses of the window origin that the sidecar did not
/// bind and forwards them to the sidecar.
///
/// The window loads a hostname such as `localhost`, which can resolve to both
/// `::1` and `127.0.0.1`. The sidecar binds only one of them. Without this
/// guard another local user can bind the other address and answer the app
/// origin. Holding every non-bind loopback address for the app lifetime closes
/// that gap even if the webview resolves the hostname again for later requests.
pub struct LoopbackGuard {
    listeners: Vec<TcpListener>,
    stop: Arc<AtomicBool>,
}

impl LoopbackGuard {
    /// Bind every loopback address that `url_host` resolves to except
    /// `bind_addr`, and forward those connections to `bind_addr`.
    pub fn install(url_host: &str, url_port: u16, bind_addr: SocketAddr) -> Result<Self, String> {
        let resolved = (url_host, url_port)
            .to_socket_addrs()
            .map_err(|error| format!("resolve {url_host}:{url_port}: {error}"))?;

        let mut listeners = Vec::new();
        for addr in resolved {
            if !addr.ip().is_loopback() || addr == bind_addr {
                continue;
            }
            let listener = TcpListener::bind(addr)
                .map_err(|error| format!("loopback address {addr} is already in use: {error}"))?;
            listener
                .set_nonblocking(true)
                .map_err(|error| format!("configure loopback listener {addr}: {error}"))?;
            listeners.push(listener);
        }

        let stop = Arc::new(AtomicBool::new(false));
        for listener in &listeners {
            let listener = listener
                .try_clone()
                .map_err(|error| format!("clone loopback listener: {error}"))?;
            let stop = stop.clone();
            thread::spawn(move || accept_loop(listener, bind_addr, stop));
        }

        Ok(Self { listeners, stop })
    }
}

impl Drop for LoopbackGuard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.listeners.clear();
    }
}

fn accept_loop(listener: TcpListener, target: SocketAddr, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((client, _)) => {
                thread::spawn(move || proxy(client, target));
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(25));
            }
            Err(_) => break,
        }
    }
}

fn proxy(client: TcpStream, target: SocketAddr) {
    let Ok(server) = TcpStream::connect(target) else {
        return;
    };
    let _ = client.set_nodelay(true);
    let _ = server.set_nodelay(true);
    let Ok(mut client_read) = client.try_clone() else {
        return;
    };
    let Ok(mut server_read) = server.try_clone() else {
        return;
    };
    let Ok(mut client_write) = client.try_clone() else {
        return;
    };
    let Ok(mut server_write) = server.try_clone() else {
        return;
    };

    let up = thread::spawn(move || {
        let _ = io::copy(&mut client_read, &mut server_write);
        let _ = server_write.shutdown(Shutdown::Write);
    });
    let down = thread::spawn(move || {
        let _ = io::copy(&mut server_read, &mut client_write);
        let _ = client_write.shutdown(Shutdown::Write);
    });
    let _ = up.join();
    let _ = down.join();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    fn alternate_loopback(url_port: u16, bind_addr: SocketAddr) -> Option<SocketAddr> {
        ("localhost", url_port)
            .to_socket_addrs()
            .ok()?
            .find(|addr| addr.ip().is_loopback() && *addr != bind_addr)
    }

    #[test]
    fn refuses_when_alternate_loopback_is_occupied() {
        let sidecar = TcpListener::bind("127.0.0.1:0").expect("bind sidecar");
        let port = sidecar.local_addr().expect("sidecar address").port();
        let bind_addr = SocketAddr::from(([127, 0, 0, 1], port));
        let Some(alternate) = alternate_loopback(port, bind_addr) else {
            return;
        };
        let Ok(occupied) = TcpListener::bind(alternate) else {
            return;
        };

        let result = LoopbackGuard::install("localhost", port, bind_addr);
        assert!(result.is_err(), "guard must refuse an occupied loopback");
        drop(occupied);
    }

    #[test]
    fn proxies_alternate_loopback_to_bind_address() {
        let sidecar = TcpListener::bind("127.0.0.1:0").expect("bind sidecar");
        let port = sidecar.local_addr().expect("sidecar address").port();
        let bind_addr = SocketAddr::from(([127, 0, 0, 1], port));
        let Some(alternate) = alternate_loopback(port, bind_addr) else {
            return;
        };

        let guard = LoopbackGuard::install("localhost", port, bind_addr).expect("install guard");
        let echo = thread::spawn(move || {
            let (mut stream, _) = sidecar.accept().expect("accept sidecar connection");
            let mut buf = [0u8; 4];
            stream.read_exact(&mut buf).expect("read request");
            stream.write_all(&buf).expect("write response");
        });

        let mut client = TcpStream::connect(alternate).expect("connect alternate loopback");
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set timeout");
        client.write_all(b"ping").expect("write ping");
        let mut buf = [0u8; 4];
        client.read_exact(&mut buf).expect("read pong");
        assert_eq!(&buf, b"ping");

        drop(guard);
        let _ = echo.join();
    }
}

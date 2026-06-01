//! Admin server for metrics and health checks.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use metrics_exporter_prometheus::PrometheusHandle;
use tracing::{error, info};

/// Spawns a background thread listening for HTTP health checks and metrics.
///
/// Binds to the provided `bind_addr` and responds to `GET /health`, `/ready`, `/status`, and `/metrics`.
/// The server returns 200 OK if `last_tick_epoch_secs` is within 5 seconds of now.
pub fn spawn_admin_server(
    bind_addr: String,
    last_tick_epoch_secs: Arc<AtomicU64>,
    startup_complete: Arc<AtomicBool>,
    prometheus_handle: Option<PrometheusHandle>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let start_time = SystemTime::now();

        let listener = match TcpListener::bind(&bind_addr) {
            Ok(l) => l,
            Err(e) => {
                error!(error = %e, addr = %bind_addr, "admin server bind failed");
                return;
            }
        };

        info!(addr = %bind_addr, "admin server listening");

        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };

            let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(1)));
            let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(1)));

            let mut buf = [0; 1024];
            if let Ok(n) = stream.read(&mut buf) {
                let req = &buf[..n];
                if req.starts_with(b"GET /health ") {
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();

                    let last_tick = last_tick_epoch_secs.load(Ordering::Relaxed);

                    let is_healthy = if last_tick == 0 {
                        // If no tick yet, allow up to 60 seconds for startup
                        start_time.elapsed().unwrap_or_default().as_secs() <= 60
                    } else {
                        // Otherwise, require a tick within the last 5 seconds
                        now <= last_tick + 5
                    };

                    if is_healthy {
                        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 15\r\n\r\n{\"status\":\"ok\"}");
                    } else {
                        let _ = stream.write_all(
                            b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 5\r\n\r\nSTALE",
                        );
                    }
                } else if req.starts_with(b"GET /ready ") {
                    if startup_complete.load(Ordering::SeqCst) {
                        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
                    } else {
                        let _ = stream.write_all(
                            b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n",
                        );
                    }
                } else if req.starts_with(b"GET /status ") {
                    let version = env!("CARGO_PKG_VERSION");
                    let is_ready = startup_complete.load(Ordering::SeqCst);
                    let status = if is_ready { "ready" } else { "starting" };
                    let payload = format!(
                        r#"{{"status":"{}","ready":{},"version":"{}"}}"#,
                        status, is_ready, version
                    );
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        payload.len(),
                        payload
                    );
                } else if req.starts_with(b"GET /metrics ") {
                    if let Some(ref handle) = prometheus_handle {
                        let payload = handle.render();
                        let _ = write!(
                            stream,
                            "HTTP/1.1 200 OK\r\nContent-Type: text/plain; version=0.0.4\r\nContent-Length: {}\r\n\r\n{}",
                            payload.len(),
                            payload
                        );
                    } else {
                        let _ = stream
                            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
                    }
                } else {
                    let _ =
                        stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpStream;
    use std::time::Duration;

    fn get_free_port() -> u16 {
        TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    fn read_response(mut stream: TcpStream) -> String {
        let mut response = String::new();
        let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
        stream.read_to_string(&mut response).unwrap_or_default();
        response
    }

    #[test]
    fn test_admin_endpoints() {
        let port = get_free_port();
        let bind_addr = format!("127.0.0.1:{}", port);

        let last_tick = Arc::new(AtomicU64::new(0));
        let startup_complete = Arc::new(AtomicBool::new(false));

        let _handle = spawn_admin_server(
            bind_addr.clone(),
            Arc::clone(&last_tick),
            Arc::clone(&startup_complete),
            None,
        );

        // Wait for the server to start
        let mut connected = false;
        for _ in 0..50 {
            if TcpStream::connect(&bind_addr).is_ok() {
                connected = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(connected, "admin server failed to start in time");

        // Test /ready before startup is complete
        let mut stream = TcpStream::connect(&bind_addr).unwrap();
        stream.write_all(b"GET /ready HTTP/1.1\r\n\r\n").unwrap();
        let resp = read_response(stream);
        assert!(resp.starts_with("HTTP/1.1 503"));

        // Test /ready after startup is complete
        startup_complete.store(true, Ordering::SeqCst);
        let mut stream = TcpStream::connect(&bind_addr).unwrap();
        stream.write_all(b"GET /ready HTTP/1.1\r\n\r\n").unwrap();
        let resp = read_response(stream);
        assert!(resp.starts_with("HTTP/1.1 200"));

        // Test /status
        let mut stream = TcpStream::connect(&bind_addr).unwrap();
        stream.write_all(b"GET /status HTTP/1.1\r\n\r\n").unwrap();
        let resp = read_response(stream);
        assert!(resp.starts_with("HTTP/1.1 200"));
        assert!(resp.contains("\"ready\":true"));
        assert!(resp.contains("\"status\":\"ready\""));

        // Test /health when healthy (0 tick)
        let mut stream = TcpStream::connect(&bind_addr).unwrap();
        stream.write_all(b"GET /health HTTP/1.1\r\n\r\n").unwrap();
        let resp = read_response(stream);
        assert!(resp.starts_with("HTTP/1.1 200"));
        assert!(resp.contains("{\"status\":\"ok\"}"));

        // Test /health when healthy (recent tick)
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        last_tick.store(now, Ordering::Relaxed);
        let mut stream = TcpStream::connect(&bind_addr).unwrap();
        stream.write_all(b"GET /health HTTP/1.1\r\n\r\n").unwrap();
        let resp = read_response(stream);
        assert!(resp.starts_with("HTTP/1.1 200"));

        // Test /health when stale (tick > 5 seconds ago)
        last_tick.store(now.saturating_sub(10), Ordering::Relaxed);
        let mut stream = TcpStream::connect(&bind_addr).unwrap();
        stream.write_all(b"GET /health HTTP/1.1\r\n\r\n").unwrap();
        let resp = read_response(stream);
        assert!(resp.starts_with("HTTP/1.1 503"));
    }
}

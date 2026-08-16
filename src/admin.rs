//! Admin server for metrics and health checks.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Sender, SyncSender};
use std::time::{SystemTime, UNIX_EPOCH};

use metrics_exporter_prometheus::PrometheusHandle;
use serde::Deserialize;
use tracing::{error, info};

/// Correlation and scenario-time data supplied by an external orchestrator.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ExternalStepCommand {
    /// Stable identity for one scenario execution.
    pub run_id: String,
    /// Timeline generation, incremented after a seek or branch.
    pub timeline_epoch: u32,
    /// Monotonic advancing tick within the timeline generation.
    pub tick_id: u64,
    /// Scenario time represented after this step completes.
    pub t_us: u64,
    /// Fixed scenario-time increment for this step.
    pub dt_us: u64,
    /// Absolute UTC origin used for DIS and UCI timestamps.
    pub scenario_epoch_utc: String,
}

/// Internal request that lets HTTP wait for completed dynamics and publishing.
pub struct ExternalStepRequest {
    /// Step metadata decoded at the transport boundary.
    pub command: ExternalStepCommand,
    /// One-shot completion returned by the simulation-owning main thread.
    pub completion: SyncSender<Result<(), String>>,
}

/// Spawns a background thread listening for HTTP health checks and metrics.
///
/// Binds to the provided `bind_addr` and responds to health/metrics requests.
/// When `external_step_sender` is present, `POST /control/step` forwards one
/// correlated request and responds only after the simulation finishes it.
/// The server returns 200 OK if `last_tick_epoch_secs` is within 5 seconds of now.
pub fn spawn_admin_server(
    bind_addr: String,
    last_tick_epoch_secs: Arc<AtomicU64>,
    startup_complete: Arc<AtomicBool>,
    prometheus_handle: Option<PrometheusHandle>,
    external_step_sender: Option<Sender<ExternalStepRequest>>,
    externally_stepped: bool,
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

            if let Ok(request) = read_http_request(&mut stream) {
                let req = request.as_slice();
                if req.starts_with(b"GET /health ") {
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();

                    let last_tick = last_tick_epoch_secs.load(Ordering::Relaxed);

                    let is_healthy = if externally_stepped {
                        // Waiting for the next authoritative step is healthy,
                        // including while the ecosystem clock is paused.
                        startup_complete.load(Ordering::SeqCst)
                    } else if last_tick == 0 {
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
                } else if req.starts_with(b"POST /control/step ") {
                    handle_external_step(&mut stream, req, external_step_sender.as_ref());
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

fn read_http_request(stream: &mut impl Read) -> std::io::Result<Vec<u8>> {
    const MAX_REQUEST_BYTES: usize = 64 * 1024;
    let mut request = Vec::with_capacity(4096);
    let mut chunk = [0_u8; 4096];
    loop {
        let count = stream.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        request.extend_from_slice(&chunk[..count]);
        if request.len() > MAX_REQUEST_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "HTTP request exceeds admin limit",
            ));
        }
        let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let headers = String::from_utf8_lossy(&request[..header_end]);
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .unwrap_or(0);
        if request.len() >= header_end + 4 + content_length {
            break;
        }
    }
    Ok(request)
}

fn handle_external_step(
    stream: &mut impl Write,
    request: &[u8],
    sender: Option<&Sender<ExternalStepRequest>>,
) {
    let Some(sender) = sender else {
        write_http_response(stream, "409 Conflict", "external stepping is disabled");
        return;
    };
    let Some(body_offset) = request.windows(4).position(|window| window == b"\r\n\r\n") else {
        write_http_response(stream, "400 Bad Request", "missing HTTP request body");
        return;
    };
    let body = &request[body_offset + 4..];
    let command = match serde_json::from_slice::<ExternalStepCommand>(body) {
        Ok(command) => command,
        Err(error) => {
            write_http_response(stream, "400 Bad Request", &error.to_string());
            return;
        }
    };
    let (completion_sender, completion_receiver) = std::sync::mpsc::sync_channel(1);
    let request = ExternalStepRequest {
        command,
        completion: completion_sender,
    };
    if sender.send(request).is_err() {
        write_http_response(
            stream,
            "503 Service Unavailable",
            "simulation loop unavailable",
        );
        return;
    }
    match completion_receiver.recv_timeout(std::time::Duration::from_secs(30)) {
        Ok(Ok(())) => write_http_response(stream, "200 OK", "{\"status\":\"completed\"}"),
        Ok(Err(detail)) => write_http_response(stream, "409 Conflict", &detail),
        Err(_) => write_http_response(stream, "504 Gateway Timeout", "simulation step timed out"),
    }
}

fn write_http_response(stream: &mut impl Write, status: &str, body: &str) {
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
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
            None,
            false,
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

    #[test]
    fn external_step_endpoint_waits_for_simulation_completion() {
        let port = get_free_port();
        let bind_addr = format!("127.0.0.1:{port}");
        let last_tick = Arc::new(AtomicU64::new(0));
        let startup_complete = Arc::new(AtomicBool::new(true));
        let (step_sender, step_receiver) = std::sync::mpsc::channel();
        let _handle = spawn_admin_server(
            bind_addr.clone(),
            last_tick,
            startup_complete,
            None,
            Some(step_sender),
            true,
        );

        for _ in 0..50 {
            if TcpStream::connect(&bind_addr).is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let client = std::thread::spawn(move || {
            let body = r#"{"run_id":"run-1","timeline_epoch":2,"tick_id":7,"t_us":1400000,"dt_us":200000,"scenario_epoch_utc":"2026-01-01T00:00:00Z"}"#;
            let mut stream = TcpStream::connect(bind_addr).unwrap();
            write!(
                stream,
                "POST /control/step HTTP/1.1\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
            read_response(stream)
        });

        let request = step_receiver.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(request.command.tick_id, 7);
        request.completion.send(Ok(())).unwrap();
        assert!(client.join().unwrap().starts_with("HTTP/1.1 200"));
    }
}

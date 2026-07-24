#![allow(clippy::result_large_err)]

//! OWP connection manager behavior tests.

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use supercell::entity::{EntityState, TimedEntityState};
use supercell::owp::{OwpPublisherConfig, OwpPublisherHandle};
use time::macros::datetime;
use tokio::net::TcpListener;
use tokio::runtime::Builder;
use tokio_tungstenite::accept_hdr_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use uuid::Uuid;

fn dummy_uuid() -> Uuid {
    Uuid::parse_str("00000000-0000-1000-8000-000000000001").unwrap()
}

fn short_backoff_config(ws_url: String) -> OwpPublisherConfig {
    OwpPublisherConfig::new(
        ws_url,
        "supercell",
        dummy_uuid(),
        dummy_uuid(),
        dummy_uuid(),
        sleet_types::uci::v2_5::ClassificationEnum::U,
        sleet_types::uci::v2_5::OwnerProducerEnum::Usa,
        10.0,
        2.0,
    )
    .with_retry_backoff(Duration::from_millis(10), Duration::from_millis(20))
}

fn unused_local_ws_url() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("test should bind an ephemeral localhost port");
    let addr = listener
        .local_addr()
        .expect("test listener should report its local address");
    drop(listener);
    format!("ws://{addr}/owp")
}

fn spawn_drop_server(accept_count: usize) -> (String, mpsc::Receiver<()>, thread::JoinHandle<()>) {
    let (addr_tx, addr_rx) = mpsc::channel();
    let (accepted_tx, accepted_rx) = mpsc::channel();

    let server = thread::spawn(move || {
        let runtime = Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
            .expect("test runtime should build");

        runtime.block_on(async move {
            let listener = TcpListener::bind("127.0.0.1:0")
                .await
                .expect("test WebSocket listener should bind");
            let addr = listener
                .local_addr()
                .expect("test WebSocket listener should report its address");
            addr_tx
                .send(format!("ws://{addr}/owp"))
                .expect("test should receive listener address");

            for _ in 0..accept_count {
                let (stream, _) = listener
                    .accept()
                    .await
                    .expect("test WebSocket listener should accept TCP connection");
                let websocket = tokio_tungstenite::accept_async(stream)
                    .await
                    .expect("test WebSocket handshake should complete");
                accepted_tx
                    .send(())
                    .expect("test should receive accept notification");
                drop(websocket);
            }
        });
    });

    let ws_url = addr_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("test WebSocket server should report its URL");

    (ws_url, accepted_rx, server)
}

fn spawn_counting_server() -> (String, mpsc::Receiver<String>, thread::JoinHandle<()>) {
    let (addr_tx, addr_rx) = mpsc::channel();
    let (msg_tx, msg_rx) = mpsc::channel();

    let server = thread::spawn(move || {
        let runtime = Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
            .expect("test runtime should build");

        runtime.block_on(async move {
            let listener = TcpListener::bind("127.0.0.1:0")
                .await
                .expect("test WebSocket listener should bind");
            let addr = listener
                .local_addr()
                .expect("test WebSocket listener should report its address");
            addr_tx
                .send(format!("ws://{addr}/owp"))
                .expect("test should receive listener address");

            let (stream, _) = listener
                .accept()
                .await
                .expect("test WebSocket listener should accept TCP connection");

            let mut ws = accept_hdr_async(stream, |_: &Request, mut response: Response| {
                response
                    .headers_mut()
                    .insert("Sec-WebSocket-Protocol", "owp".parse().unwrap());
                Ok(response)
            })
            .await
            .expect("test WebSocket handshake should complete");

            if let Some(Ok(Message::Text(text))) = ws.next().await {
                assert!(text.starts_with("INIT "));
                let info = "INFO {\"version\":\"1.0\",\"server_id\":\"mock\",\"uuids\":{\"system\":\"s\",\"service\":\"v\"},\"system_label\":\"mock\"}";
                ws.send(Message::Text(info.into())).await.unwrap();
            }

            while let Some(Ok(msg)) = ws.next().await {
                if let Message::Text(t) = msg {
                    let _ = msg_tx.send(t.to_string());
                }
            }
        });
    });

    let ws_url = addr_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("test WebSocket server should report its URL");

    (ws_url, msg_rx, server)
}

#[test]
fn state_updates_retain_latest_entity_state() {
    let config = short_backoff_config(unused_local_ws_url());
    let handle = OwpPublisherHandle::spawn(
        &config,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )
    .expect("OWP manager should spawn even when endpoint is unavailable");
    let mut state_rx = handle.subscribe_state();

    handle.update_timed_entity_state(TimedEntityState {
        state: EntityState {
            entity_id: 1,
            marking: "first".to_string(),
            ..EntityState::default()
        },
        scenario_time: datetime!(2026-01-01 0:00 UTC),
        tick: 7,
    });
    assert!(
        state_rx
            .has_changed()
            .expect("state channel should stay open"),
        "state receiver should observe the first update"
    );
    let first = state_rx
        .borrow_and_update()
        .clone()
        .expect("first state should be present");
    assert_eq!(first.state.entity_id, 1);
    assert_eq!(first.state.marking, "first");
    assert_eq!(first.scenario_time, datetime!(2026-01-01 0:00 UTC));
    assert_eq!(first.tick, 7);

    handle.update_entity_state(EntityState {
        entity_id: 2,
        marking: "latest".to_string(),
        ..EntityState::default()
    });
    assert!(
        state_rx
            .has_changed()
            .expect("state channel should stay open"),
        "state receiver should observe the latest update"
    );
    let latest = state_rx
        .borrow_and_update()
        .clone()
        .expect("latest state should be present");
    assert_eq!(latest.state.entity_id, 2);
    assert_eq!(latest.state.marking, "latest");
}

#[test]
fn unavailable_endpoint_does_not_panic_main_thread() {
    let config = short_backoff_config(unused_local_ws_url());
    let handle = OwpPublisherHandle::spawn(
        &config,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )
    .expect("OWP manager should spawn when endpoint is unavailable");

    thread::sleep(Duration::from_millis(50));
    handle.update_entity_state(EntityState {
        entity_id: 42,
        ..EntityState::default()
    });
}

#[test]
fn dropped_websocket_connection_reconnects() {
    let (ws_url, accepted_rx, server) = spawn_drop_server(2);
    let config = short_backoff_config(ws_url);
    let handle = OwpPublisherHandle::spawn(
        &config,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )
    .expect("OWP manager should spawn for local WebSocket server");

    accepted_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("OWP manager should establish the initial WebSocket connection");
    accepted_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("OWP manager should reconnect after the server drops the connection");

    drop(handle);
    server
        .join()
        .expect("test WebSocket server thread should exit cleanly");
}

#[test]
fn owp_publish_rate_limiting() {
    let (ws_url, msg_rx, server) = spawn_counting_server();
    // Use an explicit config with 10 Hz position and 2 Hz PRD
    let config = short_backoff_config(ws_url);
    let handle = OwpPublisherHandle::spawn(
        &config,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )
    .unwrap();

    // Pump state at 100 Hz for 500 ms
    let start = std::time::Instant::now();
    let mut i = 0;
    while start.elapsed() < Duration::from_millis(500) {
        handle.update_entity_state(EntityState {
            entity_id: i,
            ..EntityState::default()
        });
        i += 1;
        thread::sleep(Duration::from_millis(10));
    }

    // Give some time for messages to arrive
    thread::sleep(Duration::from_millis(100));

    let mut pos_count = 0;
    let mut sys_count = 0;
    let mut route_count = 0;

    // Read all available messages
    while let Ok(msg) = msg_rx.try_recv() {
        if msg.starts_with("PUB mission.position-report") {
            pos_count += 1;
        } else if msg.starts_with("PUB mission.system-status") {
            sys_count += 1;
        } else if msg.starts_with("PUB mission.route-plan") {
            route_count += 1;
        }
    }

    assert!(
        (3..=8).contains(&pos_count),
        "position updates not rate limited correctly: {} messages",
        pos_count
    );
    assert!(
        (0..=3).contains(&sys_count),
        "system status updates not rate limited correctly: {} messages",
        sys_count
    );
    assert_eq!(
        route_count, 0,
        "route plan should not be published with empty waypoints"
    );

    drop(handle);
    server
        .join()
        .expect("test WebSocket server thread should exit cleanly");
}

#[test]
fn owp_publish_route_plan_with_waypoints() {
    let (ws_url, msg_rx, server) = spawn_counting_server();
    // Use an explicit config with 10 Hz position and 2 Hz PRD
    let config = short_backoff_config(ws_url);
    let handle = OwpPublisherHandle::spawn(
        &config,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
    )
    .unwrap();

    // Pump state at 100 Hz for 500 ms with waypoints to trigger route plan publication
    let start = std::time::Instant::now();
    let mut i = 0;
    while start.elapsed() < Duration::from_millis(500) {
        handle.update_entity_state(EntityState {
            entity_id: i,
            waypoints: vec![supercell::config::Waypoint {
                latitude_deg: 35.0,
                longitude_deg: -118.0,
                altitude_m: 1000.0,
            }],
            ..EntityState::default()
        });
        i += 1;
        thread::sleep(Duration::from_millis(10));
    }

    // Give some time for messages to arrive
    thread::sleep(Duration::from_millis(100));

    let mut route_count = 0;

    // Read all available messages
    while let Ok(msg) = msg_rx.try_recv() {
        if msg.starts_with("PUB mission.route-plan") {
            route_count += 1;
        }
    }

    assert!(
        (1..=5).contains(&route_count),
        "route plan updates: {} messages",
        route_count
    );

    drop(handle);
    server
        .join()
        .expect("test WebSocket server thread should exit cleanly");
}

#[test]
fn connect_async_aborts_on_shutdown() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{addr}/owp");

    let config = short_backoff_config(ws_url);

    // Spawn the manager
    let handle = OwpPublisherHandle::spawn(
        &config,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )
    .unwrap();

    // Accept the TCP connection but don't perform the websocket handshake.
    // This will cause connect_async in the manager to hang.
    let (_stream, _) = listener.accept().unwrap();

    let start = std::time::Instant::now();

    // Drop the handle, which should trigger shutdown
    drop(handle);

    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_millis(500),
        "Shutdown took too long: {:?}",
        elapsed
    );
}

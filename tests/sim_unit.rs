//! Simulation unit tests using `FdmHandle` test doubles.
//!
//! These tests validate current simulation contracts without requiring a JSBSim
//! binary. Coverage focuses on active/dead lifecycle behavior, DIS publishing,
//! and FlightGear bridge input handling in the runtime tick loop.

use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use supercell::config::{DisConfig, FlightGearConfig, ResolvedTimeConfig, Waypoint};
use supercell::dis::DisPublisher;
use supercell::entity::{EntityState, EntityStatus};
use supercell::fdm::FdmHandle;
use supercell::flightgear::{FG_NET_CTRLS_VERSION, FgNetCtrls, FlightGearBridge, read_f64_at};
use supercell::sim::{RuntimeEntity, Simulation};
use supercell::time::TimeMode;
use time::macros::datetime;

type CapturedProperties = Arc<Mutex<Vec<(String, f64)>>>;

fn stepped_time_config(simulation_hz: f64) -> ResolvedTimeConfig {
    ResolvedTimeConfig {
        mode: TimeMode::Stepped,
        epoch: datetime!(2026-01-01 0:00 UTC),
        simulation_hz,
        max_wall_publish_hz: None,
    }
}

fn time_config(mode: TimeMode, simulation_hz: f64) -> ResolvedTimeConfig {
    ResolvedTimeConfig {
        mode,
        ..stepped_time_config(simulation_hz)
    }
}

/// Mock `FdmHandle` that counts steps, returns a canned state, and captures property writes.
struct MockFdmHandle {
    entity_id: u16,
    step_count: Arc<AtomicU32>,
    properties: CapturedProperties,
}

impl MockFdmHandle {
    fn new(entity_id: u16) -> (Self, Arc<AtomicU32>, CapturedProperties) {
        let step_count = Arc::new(AtomicU32::new(0));
        let properties = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                entity_id,
                step_count: Arc::clone(&step_count),
                properties: Arc::clone(&properties),
            },
            step_count,
            properties,
        )
    }
}

impl FdmHandle for MockFdmHandle {
    fn start(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    fn step(&mut self, _dt_sec: f64) -> anyhow::Result<()> {
        self.step_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn read_state(&mut self) -> anyhow::Result<EntityState> {
        Ok(EntityState {
            entity_id: self.entity_id,
            site_id: 1,
            application_id: 1,
            force_id: 1,
            latitude_deg: 35.0,
            longitude_deg: -117.0,
            altitude_m: 3000.0,
            altitude_msl_m: 3000.0,
            terrain_elevation_m: 900.0,
            ..Default::default()
        })
    }

    fn set_property(&mut self, name: &str, value: f64) -> anyhow::Result<()> {
        self.properties
            .lock()
            .expect("properties lock poisoned")
            .push((name.to_string(), value));
        Ok(())
    }

    fn get_property(&mut self, _name: &str) -> anyhow::Result<f64> {
        Ok(0.0)
    }
}

/// Mock `FdmHandle` that fails every step call.
struct FailingStepFdmHandle {
    step_count: Arc<AtomicU32>,
}

impl FailingStepFdmHandle {
    fn new() -> (Self, Arc<AtomicU32>) {
        let step_count = Arc::new(AtomicU32::new(0));
        (
            Self {
                step_count: Arc::clone(&step_count),
            },
            step_count,
        )
    }
}

impl FdmHandle for FailingStepFdmHandle {
    fn start(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    fn step(&mut self, _dt_sec: f64) -> anyhow::Result<()> {
        self.step_count.fetch_add(1, Ordering::SeqCst);
        anyhow::bail!("simulated step failure")
    }

    fn read_state(&mut self) -> anyhow::Result<EntityState> {
        anyhow::bail!("read_state must not be called after step failure")
    }

    fn set_property(&mut self, _name: &str, _value: f64) -> anyhow::Result<()> {
        Ok(())
    }

    fn get_property(&mut self, _name: &str) -> anyhow::Result<f64> {
        Ok(0.0)
    }
}

/// Mock `FdmHandle` that fails every control write while tracking step calls.
struct FailingSetPropertyFdmHandle {
    entity_id: u16,
    step_count: Arc<AtomicU32>,
}

impl FailingSetPropertyFdmHandle {
    fn new(entity_id: u16) -> (Self, Arc<AtomicU32>) {
        let step_count = Arc::new(AtomicU32::new(0));
        (
            Self {
                entity_id,
                step_count: Arc::clone(&step_count),
            },
            step_count,
        )
    }
}

impl FdmHandle for FailingSetPropertyFdmHandle {
    fn start(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    fn step(&mut self, _dt_sec: f64) -> anyhow::Result<()> {
        self.step_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn read_state(&mut self) -> anyhow::Result<EntityState> {
        Ok(EntityState {
            entity_id: self.entity_id,
            site_id: 1,
            application_id: 1,
            force_id: 1,
            latitude_deg: 35.0,
            longitude_deg: -117.0,
            altitude_m: 3000.0,
            altitude_msl_m: 3000.0,
            terrain_elevation_m: 900.0,
            ..Default::default()
        })
    }

    fn set_property(&mut self, name: &str, _value: f64) -> anyhow::Result<()> {
        anyhow::bail!("simulated control write failure for {name}")
    }

    fn get_property(&mut self, _name: &str) -> anyhow::Result<f64> {
        Ok(0.0)
    }
}

/// Mock `FdmHandle` that succeeds for `fail_after` steps and then fails.
struct DelayedFailFdmHandle {
    entity_id: u16,
    step_count: Arc<AtomicU32>,
    fail_after: u32,
}

impl DelayedFailFdmHandle {
    fn new(entity_id: u16, fail_after: u32) -> (Self, Arc<AtomicU32>) {
        let step_count = Arc::new(AtomicU32::new(0));
        (
            Self {
                entity_id,
                step_count: Arc::clone(&step_count),
                fail_after,
            },
            step_count,
        )
    }
}

impl FdmHandle for DelayedFailFdmHandle {
    fn start(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    fn step(&mut self, _dt_sec: f64) -> anyhow::Result<()> {
        let prior = self.step_count.fetch_add(1, Ordering::SeqCst);
        if prior >= self.fail_after {
            anyhow::bail!("simulated delayed failure")
        }
        Ok(())
    }

    fn read_state(&mut self) -> anyhow::Result<EntityState> {
        Ok(EntityState {
            entity_id: self.entity_id,
            site_id: 1,
            application_id: 1,
            force_id: 1,
            latitude_deg: 35.0,
            longitude_deg: -117.0,
            altitude_m: 3000.0,
            altitude_msl_m: 3000.0,
            terrain_elevation_m: 900.0,
            ..Default::default()
        })
    }

    fn set_property(&mut self, _name: &str, _value: f64) -> anyhow::Result<()> {
        Ok(())
    }

    fn get_property(&mut self, _name: &str) -> anyhow::Result<f64> {
        Ok(0.0)
    }
}

/// Mock `FdmHandle` that always returns the same state while capturing property writes.
struct StaticStateFdmHandle {
    state: EntityState,
    properties: CapturedProperties,
}

impl StaticStateFdmHandle {
    fn new(state: EntityState) -> (Self, CapturedProperties) {
        let properties = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                state,
                properties: Arc::clone(&properties),
            },
            properties,
        )
    }
}

impl FdmHandle for StaticStateFdmHandle {
    fn start(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    fn step(&mut self, _dt_sec: f64) -> anyhow::Result<()> {
        Ok(())
    }

    fn read_state(&mut self) -> anyhow::Result<EntityState> {
        Ok(self.state.clone())
    }

    fn set_property(&mut self, name: &str, value: f64) -> anyhow::Result<()> {
        self.properties
            .lock()
            .expect("properties lock poisoned")
            .push((name.to_string(), value));
        Ok(())
    }

    fn get_property(&mut self, _name: &str) -> anyhow::Result<f64> {
        Ok(0.0)
    }
}

fn waypoint(lat: f64, lon: f64, alt_m: f64) -> Waypoint {
    Waypoint {
        latitude_deg: lat,
        longitude_deg: lon,
        altitude_m: alt_m,
    }
}

fn flying_state(entity_id: u16) -> EntityState {
    EntityState {
        entity_id,
        site_id: 1,
        application_id: 1,
        force_id: 1,
        latitude_deg: 35.0,
        longitude_deg: -117.0,
        altitude_m: 3000.0,
        altitude_msl_m: 3000.0,
        terrain_elevation_m: 900.0,
        marking: format!("E-{entity_id}"),
        ..Default::default()
    }
}

fn fixed_state(entity_id: u16) -> EntityState {
    EntityState {
        entity_id,
        site_id: 1,
        application_id: 1,
        force_id: 3,
        latitude_deg: 34.0,
        longitude_deg: -116.0,
        altitude_m: 500.0,
        altitude_msl_m: 500.0,
        terrain_elevation_m: 500.0,
        marking: format!("S-{entity_id}"),
        is_static_entity: true,
        ..Default::default()
    }
}

fn flying_entity(
    handle: Box<dyn FdmHandle + Send>,
    state: EntityState,
    bridge: Option<FlightGearBridge>,
) -> RuntimeEntity {
    RuntimeEntity::Flying {
        handle,
        state,
        status: EntityStatus::Active,
        waypoints: vec![waypoint(36.0, -118.0, 3100.0)],
        active_wp: 0,
        bridge,
        prev_ecef_vel: None,
        last_hdg_setpoint: None,
        override_aggression: 5.0,
        autopilot_threshold: 0.05,
        override_timeout_secs: 1.0,
        last_fg_ctrls_at: None,
    }
}

fn make_dis_publisher(port: u16) -> DisPublisher {
    DisPublisher::new(&DisConfig {
        multicast_addr: "127.0.0.1".to_string(),
        port,
        exercise_id: 1,
        ttl: Some(1),
        multicast_iface: None,
    })
    .expect("failed to create DisPublisher")
}

fn stop_after(running: Arc<AtomicBool>, delay: Duration) {
    std::thread::spawn(move || {
        std::thread::sleep(delay);
        running.store(false, Ordering::SeqCst);
    });
}

#[test]
fn all_flying_entities_are_stepped() {
    let (fdm_a, count_a, _) = MockFdmHandle::new(1);
    let (fdm_b, count_b, _) = MockFdmHandle::new(2);
    let (fdm_c, count_c, _) = MockFdmHandle::new(3);

    let entities = vec![
        flying_entity(Box::new(fdm_a), flying_state(1), None),
        flying_entity(Box::new(fdm_b), flying_state(2), None),
        flying_entity(Box::new(fdm_c), flying_state(3), None),
    ];

    let running = Arc::new(AtomicBool::new(true));
    stop_after(Arc::clone(&running), Duration::from_millis(300));

    let mut sim = Simulation::new(entities, make_dis_publisher(0), None, 500.0, 1);
    sim.start_fdms().expect("start_fdms");
    sim.run(
        &running,
        20.0,
        0.0,
        &std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
    )
    .expect("run");

    let a = count_a.load(Ordering::SeqCst);
    let b = count_b.load(Ordering::SeqCst);
    let c = count_c.load(Ordering::SeqCst);
    assert!(
        a > 0 && b > 0 && c > 0,
        "all entities must step: a={a} b={b} c={c}"
    );
}

#[test]
fn fixed_entities_are_not_stepped() {
    let (fdm, flying_steps, _) = MockFdmHandle::new(10);
    let entities = vec![
        flying_entity(Box::new(fdm), flying_state(10), None),
        RuntimeEntity::Fixed {
            state: fixed_state(20),
            status: EntityStatus::Active,
        },
    ];

    let running = Arc::new(AtomicBool::new(true));
    stop_after(Arc::clone(&running), Duration::from_millis(200));

    let mut sim = Simulation::new(entities, make_dis_publisher(0), None, 500.0, 1);
    sim.start_fdms().expect("start_fdms");
    sim.run(
        &running,
        20.0,
        0.0,
        &std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
    )
    .expect("run");

    assert!(
        flying_steps.load(Ordering::SeqCst) > 0,
        "flying entity should be stepped",
    );
}

#[test]
fn dead_flying_entity_is_skipped() {
    let (fdm, dead_steps, _) = MockFdmHandle::new(30);
    let entities = vec![RuntimeEntity::Flying {
        handle: Box::new(fdm),
        state: flying_state(30),
        status: EntityStatus::Dead,
        waypoints: vec![waypoint(36.0, -118.0, 3100.0)],
        active_wp: 0,
        bridge: None,
        prev_ecef_vel: None,
        last_hdg_setpoint: None,
        override_aggression: 5.0,
        autopilot_threshold: 0.05,
        override_timeout_secs: 1.0,
        last_fg_ctrls_at: None,
    }];

    let running = Arc::new(AtomicBool::new(true));
    stop_after(Arc::clone(&running), Duration::from_millis(200));

    let mut sim = Simulation::new(entities, make_dis_publisher(0), None, 500.0, 1);
    sim.start_fdms().expect("start_fdms");
    sim.run(
        &running,
        20.0,
        0.0,
        &std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
    )
    .expect("run");

    assert_eq!(
        dead_steps.load(Ordering::SeqCst),
        0,
        "dead entity must not be stepped",
    );
}

#[test]
fn dis_publishes_active_flying_and_fixed_entities() {
    let receiver = UdpSocket::bind("127.0.0.1:0").expect("bind receiver");
    receiver
        .set_read_timeout(Some(Duration::from_millis(50)))
        .expect("set timeout");
    let recv_port = receiver.local_addr().expect("local addr").port();

    let (fdm, _, _) = MockFdmHandle::new(40);
    let entities = vec![
        flying_entity(Box::new(fdm), flying_state(40), None),
        RuntimeEntity::Fixed {
            state: fixed_state(41),
            status: EntityStatus::Active,
        },
    ];

    let running = Arc::new(AtomicBool::new(true));
    stop_after(Arc::clone(&running), Duration::from_millis(300));

    let mut sim = Simulation::new(entities, make_dis_publisher(recv_port), None, 500.0, 1);
    sim.start_fdms().expect("start_fdms");
    sim.run(
        &running,
        20.0,
        0.0,
        &std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
    )
    .expect("run");

    let mut pdu_count = 0u32;
    let mut count_40 = 0u32;
    let mut count_41 = 0u32;
    let mut buf = [0u8; 2048];
    while let Ok((n, _)) = receiver.recv_from(&mut buf) {
        pdu_count += 1;
        if n >= 18 {
            match u16::from_be_bytes([buf[16], buf[17]]) {
                40 => count_40 += 1,
                41 => count_41 += 1,
                _ => {}
            }
        }
    }

    assert!(pdu_count >= 4, "expected at least 4 PDUs, got {pdu_count}");
    assert!(
        count_40 >= 1,
        "expected at least one PDU for flying entity 40"
    );
    assert!(
        count_41 >= 1,
        "expected at least one PDU for fixed entity 41"
    );
}

#[test]
fn step_failure_marks_entity_dead_and_stops_future_steps() {
    let (failing_fdm, failing_steps) = FailingStepFdmHandle::new();
    let entities = vec![flying_entity(Box::new(failing_fdm), flying_state(50), None)];

    let running = Arc::new(AtomicBool::new(true));
    stop_after(Arc::clone(&running), Duration::from_millis(220));

    let mut sim = Simulation::new(entities, make_dis_publisher(0), None, 500.0, 1);
    sim.start_fdms().expect("start_fdms");
    sim.run(
        &running,
        20.0,
        0.0,
        &std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
    )
    .expect("run");

    assert_eq!(
        failing_steps.load(Ordering::SeqCst),
        1,
        "entity should step once before transition to dead",
    );
}

#[test]
fn control_write_failure_marks_only_affected_entity_dead() {
    let (failing_fdm, failing_steps) = FailingSetPropertyFdmHandle::new(55);
    let (healthy_fdm, healthy_steps, _) = MockFdmHandle::new(56);

    let entities = vec![
        flying_entity(Box::new(failing_fdm), flying_state(55), None),
        flying_entity(Box::new(healthy_fdm), flying_state(56), None),
    ];

    let running = Arc::new(AtomicBool::new(true));
    stop_after(Arc::clone(&running), Duration::from_millis(300));

    let mut sim = Simulation::new(entities, make_dis_publisher(0), None, 500.0, 1);
    sim.start_fdms().expect("start_fdms");
    sim.run(
        &running,
        20.0,
        0.0,
        &std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
    )
    .expect("run");

    assert_eq!(
        failing_steps.load(Ordering::SeqCst),
        0,
        "control-write failure should kill entity before stepping"
    );
    assert!(
        healthy_steps.load(Ordering::SeqCst) > 1,
        "sibling entity should keep stepping when one entity dies from control-write failure"
    );
}

#[test]
fn healthy_entity_keeps_running_when_sibling_fails() {
    let (failing_fdm, failing_steps) = FailingStepFdmHandle::new();
    let (healthy_fdm, healthy_steps, _) = MockFdmHandle::new(61);

    let entities = vec![
        flying_entity(Box::new(failing_fdm), flying_state(60), None),
        flying_entity(Box::new(healthy_fdm), flying_state(61), None),
    ];

    let running = Arc::new(AtomicBool::new(true));
    stop_after(Arc::clone(&running), Duration::from_millis(300));

    let mut sim = Simulation::new(entities, make_dis_publisher(0), None, 500.0, 1);
    sim.start_fdms().expect("start_fdms");
    sim.run(
        &running,
        20.0,
        0.0,
        &std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
    )
    .expect("run");

    assert_eq!(
        failing_steps.load(Ordering::SeqCst),
        1,
        "failing entity should stop after one step"
    );
    assert!(
        healthy_steps.load(Ordering::SeqCst) > 1,
        "healthy entity should continue stepping",
    );
}

#[test]
fn dis_stops_for_entity_after_it_dies() {
    let receiver = UdpSocket::bind("127.0.0.1:0").expect("bind receiver");
    receiver
        .set_read_timeout(Some(Duration::from_millis(50)))
        .expect("set timeout");
    let recv_port = receiver.local_addr().expect("local addr").port();

    let (delayed_fail_fdm, _failing_steps) = DelayedFailFdmHandle::new(70, 2);
    let (healthy_fdm, _healthy_steps, _) = MockFdmHandle::new(71);

    let entities = vec![
        flying_entity(Box::new(delayed_fail_fdm), flying_state(70), None),
        flying_entity(Box::new(healthy_fdm), flying_state(71), None),
    ];

    let running = Arc::new(AtomicBool::new(true));
    stop_after(Arc::clone(&running), Duration::from_millis(320));

    let mut sim = Simulation::new(entities, make_dis_publisher(recv_port), None, 500.0, 1);
    sim.start_fdms().expect("start_fdms");
    sim.run(
        &running,
        20.0,
        0.0,
        &std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
    )
    .expect("run");

    let mut count_70 = 0u32;
    let mut count_71 = 0u32;
    let mut buf = [0u8; 2048];
    while let Ok((n, _)) = receiver.recv_from(&mut buf) {
        if n >= 18 {
            match u16::from_be_bytes([buf[16], buf[17]]) {
                70 => count_70 += 1,
                71 => count_71 += 1,
                _ => {}
            }
        }
    }

    assert!(count_70 >= 1, "entity 70 should publish before failure");
    assert!(
        count_71 >= 4,
        "healthy entity should publish for most ticks"
    );
    assert!(
        count_70 < count_71,
        "failed entity should publish fewer PDUs"
    );
}

fn make_bridge() -> FlightGearBridge {
    let (bridge, sink) = make_bridge_with_sink();
    drop(sink);
    bridge
}

fn make_bridge_with_sink() -> (FlightGearBridge, UdpSocket) {
    let sink = UdpSocket::bind("127.0.0.1:0").expect("bind sink");
    let fdm_port = sink.local_addr().expect("sink addr").port();

    let cfg = FlightGearConfig {
        fdm_send_addr: "127.0.0.1".to_string(),
        fdm_send_port: fdm_port,
        ctrls_recv_addr: "127.0.0.1".to_string(),
        ctrls_recv_port: 0,
        override_aggression: 5,
        autopilot_threshold: 0.05,
        override_timeout_secs: 1.0,
    };

    let bridge = FlightGearBridge::new(&cfg).expect("build bridge");
    (bridge, sink)
}

fn recv_ctrls_with_retry(bridge: &FlightGearBridge) -> anyhow::Result<Option<FgNetCtrls>> {
    for _ in 0..20 {
        if let Some(ctrls) = bridge.recv_ctrls_nonblocking()? {
            return Ok(Some(ctrls));
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    Ok(None)
}

#[test]
fn flightgear_bridge_nonblocking_empty_returns_none() {
    let bridge = make_bridge();
    let received = bridge
        .recv_ctrls_nonblocking()
        .expect("empty nonblocking receive should not fail");
    assert!(received.is_none(), "expected no controls packet");
}

#[test]
fn flightgear_bridge_drops_malformed_controls_packet() {
    let bridge = make_bridge();
    let ctrls_addr = bridge.ctrls_local_addr().expect("ctrls addr");

    let sender = UdpSocket::bind("127.0.0.1:0").expect("bind sender");
    sender
        .send_to(&[0x00, 0x01, 0x02, 0x03], ctrls_addr)
        .expect("send malformed controls packet");

    let received = recv_ctrls_with_retry(&bridge).expect("malformed packet should not error");
    assert!(
        received.is_none(),
        "malformed packet must be dropped without surfacing an error"
    );
}

#[test]
fn flightgear_bridge_delivers_valid_controls_packet() {
    let bridge = make_bridge();
    let ctrls_addr = bridge.ctrls_local_addr().expect("ctrls addr");

    let mut ctrls = FgNetCtrls {
        version: FG_NET_CTRLS_VERSION,
        aileron: -0.15,
        ..FgNetCtrls::default()
    };
    ctrls.throttle[0] = 0.55;

    let sender = UdpSocket::bind("127.0.0.1:0").expect("bind sender");
    sender
        .send_to(&ctrls.encode(), ctrls_addr)
        .expect("send valid controls packet");

    let received = recv_ctrls_with_retry(&bridge)
        .expect("valid controls packet should decode")
        .expect("valid controls packet should be returned");

    assert!((received.throttle[0] - 0.55).abs() < 1e-9);
    assert!((received.aileron - (-0.15)).abs() < 1e-9);
}

#[test]
fn fg_interp_emits_finite_lat_lon_near_poles() {
    const FDM_OFF_LONGITUDE: usize = 8;
    const FDM_OFF_LATITUDE: usize = 16;

    let (bridge, sink) = make_bridge_with_sink();
    sink.set_read_timeout(Some(Duration::from_millis(400)))
        .expect("set sink timeout");

    let mut state = flying_state(98);
    state.latitude_deg = 89.999_999;
    state.longitude_deg = 45.0;
    state.velocity_north_mps = 120.0;
    state.velocity_east_mps = 120.0;

    let (fdm, _props) = StaticStateFdmHandle::new(state.clone());
    let entities = vec![flying_entity(Box::new(fdm), state, Some(bridge))];

    let running = Arc::new(AtomicBool::new(true));
    stop_after(Arc::clone(&running), Duration::from_millis(240));

    let mut sim = Simulation::new(entities, make_dis_publisher(0), None, 500.0, 1);
    sim.start_fdms().expect("start_fdms");
    sim.run(
        &running,
        20.0,
        0.0,
        &std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
    )
    .expect("run");

    let mut buf = [0u8; 512];
    let (n, _) = sink
        .recv_from(&mut buf)
        .expect("expected interpolated FGNetFDM packet");
    let payload = &buf[..n];

    let lon_rad = read_f64_at(payload, FDM_OFF_LONGITUDE);
    let lat_rad = read_f64_at(payload, FDM_OFF_LATITUDE);

    assert!(lon_rad.is_finite(), "longitude must stay finite near poles");
    assert!(lat_rad.is_finite(), "latitude must stay finite near poles");
}

#[test]
fn flightgear_throttle_input_writes_manual_throttle_command() {
    let bridge = make_bridge();
    let ctrls_addr = bridge.ctrls_local_addr().expect("ctrls addr");

    let mut ctrls = FgNetCtrls {
        version: FG_NET_CTRLS_VERSION,
        aileron: 0.2,
        elevator: 0.1,
        ..FgNetCtrls::default()
    };
    ctrls.throttle[0] = 0.62;

    let sender = UdpSocket::bind("127.0.0.1:0").expect("bind sender");
    sender
        .send_to(&ctrls.encode(), ctrls_addr)
        .expect("send controls packet");

    let (fdm, _steps, props) = MockFdmHandle::new(80);
    let entities = vec![flying_entity(Box::new(fdm), flying_state(80), Some(bridge))];

    let running = Arc::new(AtomicBool::new(true));
    stop_after(Arc::clone(&running), Duration::from_millis(250));

    let mut sim = Simulation::new(entities, make_dis_publisher(0), None, 500.0, 1);
    sim.start_fdms().expect("start_fdms");
    sim.run(
        &running,
        20.0,
        0.0,
        &std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
    )
    .expect("run");

    let props = props.lock().expect("properties lock poisoned");
    let manual_throttle_written = props
        .iter()
        .any(|(name, value)| name == "fcs/throttle-cmd-norm" && (*value - 0.62).abs() < 1e-9);

    assert!(
        manual_throttle_written,
        "manual throttle command must be written when FG throttle engages override; got {props:?}",
    );
}

#[test]
fn manual_override_persists_across_short_controls_gap() {
    let bridge = make_bridge();
    let ctrls_addr = bridge.ctrls_local_addr().expect("ctrls addr");

    let mut ctrls = FgNetCtrls {
        version: FG_NET_CTRLS_VERSION,
        ..FgNetCtrls::default()
    };
    ctrls.throttle[0] = 0.62;

    let sender = UdpSocket::bind("127.0.0.1:0").expect("bind sender");
    sender
        .send_to(&ctrls.encode(), ctrls_addr)
        .expect("send controls packet");

    let (fdm, _steps, props) = MockFdmHandle::new(95);
    let mut entity = flying_entity(Box::new(fdm), flying_state(95), Some(bridge));
    if let RuntimeEntity::Flying {
        override_timeout_secs,
        ..
    } = &mut entity
    {
        *override_timeout_secs = 1.0;
    }

    let entities = vec![entity];
    let running = Arc::new(AtomicBool::new(true));
    stop_after(Arc::clone(&running), Duration::from_millis(260));

    let mut sim = Simulation::new(entities, make_dis_publisher(0), None, 500.0, 1);
    sim.start_fdms().expect("start_fdms");
    sim.run(
        &running,
        20.0,
        0.0,
        &std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
    )
    .expect("run");

    let props = props.lock().expect("properties lock poisoned");
    let throttle_write_count = props
        .iter()
        .filter(|(name, value)| name == "fcs/throttle-cmd-norm" && (*value - 0.62).abs() < 1e-9)
        .count();

    assert!(
        throttle_write_count >= 3,
        "manual override should persist across short controls gaps; writes={throttle_write_count}, props={props:?}"
    );
}

#[test]
fn zero_override_timeout_disengages_on_first_missing_packet_tick() {
    let bridge = make_bridge();
    let ctrls_addr = bridge.ctrls_local_addr().expect("ctrls addr");

    let mut ctrls = FgNetCtrls {
        version: FG_NET_CTRLS_VERSION,
        ..FgNetCtrls::default()
    };
    ctrls.throttle[0] = 0.62;

    let sender = UdpSocket::bind("127.0.0.1:0").expect("bind sender");
    sender
        .send_to(&ctrls.encode(), ctrls_addr)
        .expect("send controls packet");

    let (fdm, _steps, props) = MockFdmHandle::new(96);
    let mut entity = flying_entity(Box::new(fdm), flying_state(96), Some(bridge));
    if let RuntimeEntity::Flying {
        override_timeout_secs,
        ..
    } = &mut entity
    {
        *override_timeout_secs = 0.0;
    }

    let entities = vec![entity];
    let running = Arc::new(AtomicBool::new(true));
    stop_after(Arc::clone(&running), Duration::from_millis(260));

    let mut sim = Simulation::new(entities, make_dis_publisher(0), None, 500.0, 1);
    sim.start_fdms().expect("start_fdms");
    sim.run(
        &running,
        20.0,
        0.0,
        &std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
    )
    .expect("run");

    let props = props.lock().expect("properties lock poisoned");
    let throttle_write_count = props
        .iter()
        .filter(|(name, _)| name == "fcs/throttle-cmd-norm")
        .count();

    assert!(
        throttle_write_count <= 1,
        "zero timeout should disengage manual override after first packet gap; writes={throttle_write_count}, props={props:?}"
    );
}

#[test]
fn low_throttle_input_updates_manual_throttle_value() {
    let bridge = make_bridge();
    let ctrls_addr = bridge.ctrls_local_addr().expect("ctrls addr");

    let sender = UdpSocket::bind("127.0.0.1:0").expect("bind sender");

    let mut engage_ctrls = FgNetCtrls {
        version: FG_NET_CTRLS_VERSION,
        ..FgNetCtrls::default()
    };
    engage_ctrls.throttle[0] = 0.50;
    sender
        .send_to(&engage_ctrls.encode(), ctrls_addr)
        .expect("send engage controls packet");

    let mut low_ctrls = FgNetCtrls {
        version: FG_NET_CTRLS_VERSION,
        ..FgNetCtrls::default()
    };
    low_ctrls.throttle[0] = 0.0;

    let low_payload = low_ctrls.encode();
    let delayed_sender = sender.try_clone().expect("clone sender");
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(80));
        delayed_sender
            .send_to(&low_payload, ctrls_addr)
            .expect("send low throttle controls packet");
    });

    let (fdm, _steps, props) = MockFdmHandle::new(97);
    let mut entity = flying_entity(Box::new(fdm), flying_state(97), Some(bridge));
    if let RuntimeEntity::Flying {
        autopilot_threshold,
        override_timeout_secs,
        ..
    } = &mut entity
    {
        *autopilot_threshold = 0.0;
        *override_timeout_secs = 1.0;
    }

    let entities = vec![entity];
    let running = Arc::new(AtomicBool::new(true));
    stop_after(Arc::clone(&running), Duration::from_millis(260));

    let mut sim = Simulation::new(entities, make_dis_publisher(0), None, 500.0, 1);
    sim.start_fdms().expect("start_fdms");
    sim.run(
        &running,
        20.0,
        0.0,
        &std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
    )
    .expect("run");

    let props = props.lock().expect("properties lock poisoned");
    let saw_idle_write = props
        .iter()
        .any(|(name, value)| name == "fcs/throttle-cmd-norm" && value.abs() < 1e-9);

    assert!(
        saw_idle_write,
        "manual throttle writes should include idle input and not retain stale prior value; props={props:?}"
    );
}

#[test]
fn stepped_mode_advances_exact_requested_tick_count() {
    let (fdm, steps, _) = MockFdmHandle::new(100);
    let entities = vec![flying_entity(Box::new(fdm), flying_state(100), None)];
    let heartbeat = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let mut sim = Simulation::new(entities, make_dis_publisher(0), None, 500.0, 100);
    sim.start_fdms().expect("start_fdms");

    sim.step_ticks(&stepped_time_config(20.0), 0.0, &heartbeat, 4)
        .expect("four explicit ticks");

    assert_eq!(steps.load(Ordering::SeqCst), 4);
    assert_eq!(sim.local_tick(), 4);
    assert_eq!(sim.local_scenario_elapsed(), Duration::from_millis(200));
}

#[test]
fn step_once_advances_one_tick_per_call() {
    let (fdm, steps, _) = MockFdmHandle::new(101);
    let entities = vec![flying_entity(Box::new(fdm), flying_state(101), None)];
    let heartbeat = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let time_config = stepped_time_config(20.0);
    let mut sim = Simulation::new(entities, make_dis_publisher(0), None, 500.0, 101);
    sim.start_fdms().expect("start_fdms");

    sim.step_once(&time_config, 0.0, &heartbeat)
        .expect("first explicit tick");
    sim.step_once(&time_config, 0.0, &heartbeat)
        .expect("second explicit tick");

    assert_eq!(steps.load(Ordering::SeqCst), 2);
    assert_eq!(sim.local_tick(), 2);
    assert_eq!(sim.local_scenario_elapsed(), Duration::from_millis(100));
}

#[test]
fn zero_explicit_ticks_do_not_advance_entities() {
    let (fdm, steps, _) = MockFdmHandle::new(102);
    let entities = vec![flying_entity(Box::new(fdm), flying_state(102), None)];
    let heartbeat = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let mut sim = Simulation::new(entities, make_dis_publisher(0), None, 500.0, 102);
    sim.start_fdms().expect("start_fdms");

    sim.step_ticks(&stepped_time_config(20.0), 0.0, &heartbeat, 0)
        .expect("zero ticks");

    assert_eq!(steps.load(Ordering::SeqCst), 0);
}

#[test]
fn fixed_tick_results_are_equivalent_across_all_time_modes() {
    // A high frequency keeps the paced cases fast without changing their fixed
    // one-millisecond scenario integration step.
    const TICKS: u64 = 5;
    const HZ: f64 = 1_000.0;

    let modes = [
        TimeMode::Realtime,
        TimeMode::Scaled { rate: 10.0 },
        TimeMode::Unpaced,
        TimeMode::Stepped,
    ];
    let mut results = Vec::new();

    // Give every mode a fresh, identical FDM double. This prevents state from a
    // previous run from hiding a mode-dependent integration or control change.
    for mode in modes {
        let (fdm, steps, properties) = MockFdmHandle::new(103);
        let entities = vec![flying_entity(Box::new(fdm), flying_state(103), None)];
        let heartbeat = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let mut sim = Simulation::new(entities, make_dis_publisher(0), None, 500.0, 103);
        sim.start_fdms().expect("start_fdms");

        sim.run_ticks_with_time(&time_config(mode, HZ), 0.0, &heartbeat, TICKS)
            .expect("bounded fixed-tick run");

        results.push((
            steps.load(Ordering::SeqCst),
            sim.local_tick(),
            sim.local_scenario_elapsed(),
            properties.lock().expect("properties lock poisoned").clone(),
        ));
    }

    // Realtime is only the baseline tuple; all four modes must deliver the same
    // steps, logical clock advancement, and ordered FDM control writes. Their
    // wall-clock completion durations are intentionally allowed to differ.
    let expected = &results[0];
    assert_eq!(expected.0, TICKS as u32);
    assert_eq!(expected.1, TICKS);
    assert_eq!(expected.2, Duration::from_millis(TICKS));
    for result in &results[1..] {
        assert_eq!(result, expected);
    }
}

#[test]
fn settle_phase_suppresses_control_writes() {
    let (fdm, _steps, props) = MockFdmHandle::new(90);
    let entities = vec![flying_entity(Box::new(fdm), flying_state(90), None)];

    let running = Arc::new(AtomicBool::new(true));
    stop_after(Arc::clone(&running), Duration::from_millis(250));

    let mut sim = Simulation::new(entities, make_dis_publisher(0), None, 500.0, 1);
    sim.start_fdms().expect("start_fdms");
    sim.run(
        &running,
        20.0,
        5.0,
        &std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
    )
    .expect("run");

    let props = props.lock().expect("properties lock poisoned");
    assert!(
        props.is_empty(),
        "settle phase should suppress control writes, got {props:?}"
    );
}

#[test]
fn control_writes_resume_after_settle_phase() {
    let (fdm, _steps, props) = MockFdmHandle::new(91);
    let entities = vec![flying_entity(Box::new(fdm), flying_state(91), None)];

    let running = Arc::new(AtomicBool::new(true));
    stop_after(Arc::clone(&running), Duration::from_millis(250));

    let mut sim = Simulation::new(entities, make_dis_publisher(0), None, 500.0, 1);
    sim.start_fdms().expect("start_fdms");
    sim.run(
        &running,
        20.0,
        0.0,
        &std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
    )
    .expect("run");

    let props = props.lock().expect("properties lock poisoned");
    assert!(
        props.iter().any(|(name, _)| name.starts_with("ap/")),
        "expected AP control writes after settle window, got {props:?}"
    );
}

#[test]
fn waypoint_sphere_uses_vertical_component_for_arrival() {
    let mut state = flying_state(92);
    state.latitude_deg = 35.0;
    state.longitude_deg = -117.0;
    state.altitude_msl_m = 3000.0;

    let (fdm, props) = StaticStateFdmHandle::new(state.clone());
    let entities = vec![RuntimeEntity::Flying {
        handle: Box::new(fdm),
        state,
        status: EntityStatus::Active,
        waypoints: vec![
            waypoint(35.0, -117.0, 3600.0),
            waypoint(35.0, -116.0, 3000.0),
        ],
        active_wp: 0,
        bridge: None,
        prev_ecef_vel: None,
        last_hdg_setpoint: None,
        override_aggression: 5.0,
        autopilot_threshold: 0.05,
        override_timeout_secs: 1.0,
        last_fg_ctrls_at: None,
    }];

    let running = Arc::new(AtomicBool::new(true));
    stop_after(Arc::clone(&running), Duration::from_millis(260));

    let mut sim = Simulation::new(entities, make_dis_publisher(0), None, 500.0, 1);
    sim.start_fdms().expect("start_fdms");
    sim.run(
        &running,
        20.0,
        0.0,
        &std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
    )
    .expect("run");

    let props = props.lock().expect("properties lock poisoned");
    let saw_next_wp_heading = props
        .iter()
        .any(|(name, value)| name == "ap/heading_setpoint" && (*value - 89.713).abs() < 0.5);
    assert!(
        !saw_next_wp_heading,
        "waypoint should not advance when only horizontal is near but vertical distance keeps entity outside sphere; got {props:?}"
    );
}

#[test]
fn waypoint_threshold_is_configurable_for_sphere_arrival() {
    let mut state = flying_state(93);
    state.latitude_deg = 35.0;
    state.longitude_deg = -117.0;
    state.altitude_msl_m = 3000.0;

    let (fdm, props) = StaticStateFdmHandle::new(state.clone());
    let entities = vec![RuntimeEntity::Flying {
        handle: Box::new(fdm),
        state,
        status: EntityStatus::Active,
        waypoints: vec![
            waypoint(35.0, -117.0, 3600.0),
            waypoint(35.0, -116.0, 3000.0),
        ],
        active_wp: 0,
        bridge: None,
        prev_ecef_vel: None,
        last_hdg_setpoint: None,
        override_aggression: 5.0,
        autopilot_threshold: 0.05,
        override_timeout_secs: 1.0,
        last_fg_ctrls_at: None,
    }];

    let running = Arc::new(AtomicBool::new(true));
    stop_after(Arc::clone(&running), Duration::from_millis(260));

    let mut sim = Simulation::new(entities, make_dis_publisher(0), None, 700.0, 1);
    sim.start_fdms().expect("start_fdms");
    sim.run(
        &running,
        20.0,
        0.0,
        &std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
    )
    .expect("run");

    let props = props.lock().expect("properties lock poisoned");
    let saw_next_wp_heading = props
        .iter()
        .any(|(name, value)| name == "ap/heading_setpoint" && (*value - 89.713).abs() < 0.5);
    assert!(
        saw_next_wp_heading,
        "larger waypoint threshold should allow 3D sphere arrival and advance to next waypoint; got {props:?}"
    );
}

#[test]
fn autopilot_threshold_controls_manual_override_engagement() {
    let bridge = make_bridge();
    let ctrls_addr = bridge.ctrls_local_addr().expect("ctrls addr");

    let mut ctrls = FgNetCtrls {
        version: FG_NET_CTRLS_VERSION,
        ..FgNetCtrls::default()
    };
    ctrls.throttle[0] = 0.62;

    let sender = UdpSocket::bind("127.0.0.1:0").expect("bind sender");
    sender
        .send_to(&ctrls.encode(), ctrls_addr)
        .expect("send controls packet");

    let (fdm, _steps, props) = MockFdmHandle::new(94);
    let mut entity = flying_entity(Box::new(fdm), flying_state(94), Some(bridge));
    if let RuntimeEntity::Flying {
        autopilot_threshold,
        ..
    } = &mut entity
    {
        *autopilot_threshold = 0.80;
    }

    let entities = vec![entity];
    let running = Arc::new(AtomicBool::new(true));
    stop_after(Arc::clone(&running), Duration::from_millis(250));

    let mut sim = Simulation::new(entities, make_dis_publisher(0), None, 500.0, 1);
    sim.start_fdms().expect("start_fdms");
    sim.run(
        &running,
        20.0,
        0.0,
        &std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
    )
    .expect("run");

    let props = props.lock().expect("properties lock poisoned");
    let manual_throttle_written = props
        .iter()
        .any(|(name, _)| name == "fcs/throttle-cmd-norm");

    assert!(
        !manual_throttle_written,
        "manual throttle command should not be written when throttle is below autopilot_threshold; got {props:?}"
    );
}

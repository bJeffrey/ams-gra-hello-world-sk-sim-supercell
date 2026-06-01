//! Integration test: full JSBSim → DIS pipeline.
//!
//! Marked `#[ignore]` because it requires a real JSBSim binary on PATH
//! (or in the PATH inside the build container).
//!
//! Run with:
//!   cargo test --test pipeline -- --ignored

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dis_rs::enumerations::DeadReckoningAlgorithm;
use dis_rs::model::PduBody;
use supercell::config::{
    DisConfig, EntityBaseConfig, EntityTypeConfig, FlyingEntityConfig, JsbsimConnectionMode,
    Waypoint,
};
use supercell::dis::{DisPublisher, build_entity_state_pdu, ned_euler_to_dis_orientation};
use supercell::entity::EntityStatus;
use supercell::fdm::{FdmHandle, JsbsimHandle};
use supercell::sim::{RuntimeEntity, Simulation};

/// Build a minimal FlyingEntityConfig that spawns JSBSim with the c172p aircraft.
fn test_entity_config() -> FlyingEntityConfig {
    FlyingEntityConfig {
        base: EntityBaseConfig {
            entity_id: 1,
            site_id: 1,
            application_id: 1,
            force_id: 1,
            name: "test-aircraft".to_string(),
            entity_type: EntityTypeConfig::default(),
        },
        aircraft: "c172x".to_string(),
        jsbsim: JsbsimConnectionMode::Spawn {
            jsbsim_root: None,
            port: None,
        },
        flight_plan: None,
    }
}

/// Build a minimal DisConfig pointing at loopback multicast.
fn test_dis_config() -> DisConfig {
    DisConfig {
        multicast_addr: "239.1.2.3".to_string(),
        port: 13000,
        exercise_id: 1,
        ttl: Some(1),
        multicast_iface: None,
    }
}

/// JSBSim-backed wrapper that records set-property calls for behavioral assertions.
struct CountingJsbsimHandle {
    inner: JsbsimHandle,
    set_names: Arc<Mutex<Vec<String>>>,
}

impl CountingJsbsimHandle {
    fn new(config: &FlyingEntityConfig, set_names: Arc<Mutex<Vec<String>>>) -> Self {
        let inner = JsbsimHandle::new(config).expect("JsbsimHandle::new should succeed");
        Self { inner, set_names }
    }
}

impl FdmHandle for CountingJsbsimHandle {
    fn start(&mut self) -> anyhow::Result<()> {
        self.inner.start()
    }

    fn step(&mut self, dt_sec: f64) -> anyhow::Result<()> {
        self.inner.step(dt_sec)
    }

    fn read_state(&mut self) -> anyhow::Result<supercell::entity::EntityState> {
        self.inner.read_state()
    }

    fn set_property(&mut self, name: &str, value: f64) -> anyhow::Result<()> {
        self.set_names
            .lock()
            .expect("set_names lock poisoned")
            .push(name.to_string());
        self.inner.set_property(name, value)
    }

    fn get_property(&mut self, name: &str) -> anyhow::Result<f64> {
        self.inner.get_property(name)
    }
}

fn stop_after(running: Arc<AtomicBool>, delay: Duration) {
    std::thread::spawn(move || {
        std::thread::sleep(delay);
        running.store(false, Ordering::SeqCst);
    });
}

/// Full pipeline: spawn JSBSim, step 10 times, assert kinematic state changes,
/// publish DIS PDU, verify Ok return.
#[test]
#[ignore = "requires JSBSim binary on PATH"]
fn test_s01_full_pipeline() {
    // Skip cleanly when JSBSim is not installed in the current runtime
    // environment (common in CI/container builder images).
    let jsbsim_available = std::process::Command::new("JSBSim")
        .arg("--version")
        .output()
        .is_ok();

    if !jsbsim_available {
        eprintln!("skipping pipeline: JSBSim binary not found on PATH");
        return;
    }

    // ── FDM ──────────────────────────────────────────────────────────────────
    let config = test_entity_config();
    let mut fdm = JsbsimHandle::new(&config)
        .expect("JsbsimHandle::new should succeed when JSBSim is available");

    const STEPS: usize = 10;
    const DT: f64 = 0.1;

    let mut states = Vec::with_capacity(STEPS);

    for i in 0..STEPS {
        fdm.step(DT)
            .unwrap_or_else(|e| panic!("step {i} failed: {e}"));
        let state = fdm
            .read_state()
            .unwrap_or_else(|e| panic!("read_state {i} failed: {e}"));
        states.push(state);
    }

    // Position should not be all-zero after stepping (JSBSim initialises to a
    // real location and the C172p should have a non-zero initial lat/lon)
    let final_state = states.last().unwrap();
    assert!(
        final_state.latitude_deg.abs() > 1e-6
            || final_state.longitude_deg.abs() > 1e-6
            || final_state.altitude_m.abs() > 0.0,
        "All position fields are zero after {STEPS} steps — JSBSim may not be running"
    );

    // ── DIS publish ───────────────────────────────────────────────────────────
    let dis_config = test_dis_config();
    let mut publisher =
        DisPublisher::new(&dis_config).expect("DisPublisher::new should succeed on loopback");

    publisher
        .publish(final_state)
        .expect("DisPublisher::publish should return Ok");

    // ── Optional: loopback readback ──────────────────────────────────────────
    // Bind a receive socket to the same multicast group and verify basic DIS
    // header bytes on the received packet (protocol version, exercise ID, PDU type).
    use std::net::{Ipv4Addr, UdpSocket};

    let recv_socket = UdpSocket::bind("0.0.0.0:13000").expect("bind recv socket");
    let multicast_addr: Ipv4Addr = dis_config.multicast_addr.parse().unwrap();
    recv_socket
        .join_multicast_v4(&multicast_addr, &Ipv4Addr::UNSPECIFIED)
        .expect("join multicast group");
    recv_socket
        .set_read_timeout(Some(std::time::Duration::from_secs(2)))
        .unwrap();

    // Publish again so the receiver can read the packet
    publisher
        .publish(final_state)
        .expect("second publish should succeed");

    let mut buf = [0u8; 2048];
    let (len, _) = recv_socket
        .recv_from(&mut buf)
        .expect("recv_from should get a PDU within 2 seconds");

    assert!(len > 12, "PDU too short: {len} bytes");
    // DIS v7: byte[0] = 7 (protocol version)
    assert_eq!(buf[0], 7, "DIS protocol version byte should be 7");
    // DIS: byte[2] = PDU type; EntityState = 1
    assert_eq!(buf[2], 1, "PDU type byte should be 1 (EntityState)");
    // Exercise ID byte[1]
    assert_eq!(
        buf[1], dis_config.exercise_id as u8,
        "exercise_id mismatch in PDU header"
    );

    let parsed = dis_rs::parse(&buf[..len]).expect("received DIS PDU should parse");
    assert_eq!(parsed.len(), 1, "expected one received PDU");

    assert_eq!(
        parsed[0].header.time_stamp & 1,
        1,
        "timestamp must be absolute (LSB=1)"
    );
    assert!(
        parsed[0].header.time_stamp > 1,
        "timestamp must be populated with a non-zero value"
    );

    let body = match &parsed[0].body {
        PduBody::EntityState(body) => body,
        other => panic!("expected EntityState body, got {:?}", other),
    };

    // Scenario entity is flying, so dead reckoning must be RVW.
    assert_eq!(
        body.dead_reckoning_parameters.algorithm,
        DeadReckoningAlgorithm::DRM_RVW_HighSpeedOrManeuveringEntityWithExtrapolationOfOrientation
    );

    // Orientation contract: DIS orientation is ECEF Euler extracted from NED input,
    // not direct yaw/pitch/roll passthrough.
    let (psi, theta, phi) = ned_euler_to_dis_orientation(
        final_state.latitude_deg,
        final_state.longitude_deg,
        final_state.yaw_deg,
        final_state.pitch_deg,
        final_state.roll_deg,
    );

    let tol = 1e-5_f32;
    assert!(
        (body.entity_orientation.psi - psi).abs() < tol,
        "psi mismatch"
    );
    assert!(
        (body.entity_orientation.theta - theta).abs() < tol,
        "theta mismatch"
    );
    assert!(
        (body.entity_orientation.phi - phi).abs() < tol,
        "phi mismatch"
    );

    // Guard against accidental NED passthrough by comparing to naive yaw in radians.
    let naive_yaw = final_state.yaw_deg.to_radians() as f32;
    assert!(
        (body.entity_orientation.psi - naive_yaw).abs() > 1e-3,
        "psi unexpectedly matches raw yaw; expected ECEF-frame orientation conversion"
    );

    // Builder-level contract and wire output should agree for the same state.
    let expected = build_entity_state_pdu(final_state, dis_config.exercise_id as u8, 0);
    let expected_body = match expected.body {
        PduBody::EntityState(body) => body,
        other => panic!("expected EntityState body, got {:?}", other),
    };
    assert_eq!(
        body.dead_reckoning_parameters.algorithm,
        expected_body.dead_reckoning_parameters.algorithm
    );
}

fn run_jsbsim_sim_for_settle_contract(settle_secs: f64) -> Vec<String> {
    let config = test_entity_config();
    let set_names = Arc::new(Mutex::new(Vec::new()));
    let mut handle = CountingJsbsimHandle::new(&config, Arc::clone(&set_names));

    let mut state = handle
        .read_state()
        .expect("read_state should succeed for simulation setup");
    state.marking = "test-aircraft".to_string();
    state.is_static_entity = false;

    let waypoints = vec![Waypoint {
        latitude_deg: state.latitude_deg + 0.01,
        longitude_deg: state.longitude_deg,
        altitude_m: state.altitude_msl_m,
    }];

    let entities = vec![RuntimeEntity::Flying {
        handle: Box::new(handle),
        state,
        status: EntityStatus::Active,
        waypoints,
        active_wp: 0,
        bridge: None,
        prev_ecef_vel: None,
        last_hdg_setpoint: None,
        override_aggression: 5.0,
        autopilot_threshold: 0.05,
        override_timeout_secs: 1.0,
        last_fg_ctrls_at: None,
    }];

    let mut simulation = Simulation::new(
        entities,
        DisPublisher::new(&test_dis_config()).expect("DisPublisher::new should succeed"),
        None,
        500.0,
        1,
    );
    simulation.start_fdms().expect("start_fdms should succeed");

    let running = Arc::new(AtomicBool::new(true));
    stop_after(Arc::clone(&running), Duration::from_millis(300));
    simulation
        .run(
            &running,
            20.0,
            settle_secs,
            &std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
        )
        .expect("simulation run should succeed");

    set_names.lock().expect("set_names lock poisoned").clone()
}

#[test]
#[ignore = "requires JSBSim binary on PATH"]
fn test_s01_settle_phase_suppresses_and_then_allows_control_writes() {
    let jsbsim_available = std::process::Command::new("JSBSim")
        .arg("--version")
        .output()
        .is_ok();

    if !jsbsim_available {
        eprintln!("skipping s01_settle contract: JSBSim binary not found on PATH");
        return;
    }

    let during_settle = run_jsbsim_sim_for_settle_contract(5.0);
    let settled = run_jsbsim_sim_for_settle_contract(0.0);

    assert!(
        during_settle.is_empty(),
        "expected no control writes during settle window, got {during_settle:?}"
    );
    assert!(
        settled.iter().any(|name| name.starts_with("ap/")),
        "expected AP control writes after settle window, got {settled:?}"
    );
}

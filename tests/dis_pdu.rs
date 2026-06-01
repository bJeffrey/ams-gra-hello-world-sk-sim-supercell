//! DIS PDU construction, serialization, and coordinate conversion tests.
//!
//! These tests do not require JSBSim.

use bytes::BytesMut;
use dis_rs::enumerations::{DeadReckoningAlgorithm, PduType};
use dis_rs::model::PduBody;
use supercell::dis::{
    build_entity_state_pdu, geodetic_to_ecef, ned_euler_to_dis_orientation, ned_to_ecef_velocity,
};
use supercell::entity::{DisEntityType, EntityState};

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn sample_entity_state() -> EntityState {
    EntityState {
        latitude_deg: 36.12,
        longitude_deg: -86.67,
        altitude_m: 1000.0,
        altitude_msl_m: 1000.0,
        terrain_elevation_m: 120.0,
        velocity_north_mps: 10.0,
        velocity_east_mps: 5.0,
        velocity_down_mps: -2.0,
        roll_deg: 1.5,
        pitch_deg: 2.5,
        yaw_deg: 45.0,
        entity_id: 7,
        site_id: 1,
        application_id: 2,
        force_id: 1, // Friendly
        entity_type: DisEntityType {
            kind: 1,        // Platform
            domain: 2,      // Air
            country: 225,   // USA
            category: 84,   // Civilian Fixed-Wing Single
            subcategory: 1, // Cessna 172
            specific: 0,
            extra: 0,
        },
        marking: "Test-7".to_string(),
        sim_time_s: 0.0,
        ..Default::default()
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

/// PDU round-trip: build → serialize → parse → assert fields survive the trip.
#[test]
fn test_pdu_round_trip() {
    let state = sample_entity_state();
    let exercise_id: u8 = 42;

    let pdu = build_entity_state_pdu(&state, exercise_id, 0);

    // Serialize into a BytesMut buffer
    let pdu_len = pdu.pdu_length() as usize;
    let mut buf = BytesMut::with_capacity(pdu_len);
    pdu.serialize(&mut buf).expect("PDU serialization failed");

    // Parse the bytes back into PDUs
    let pdus = dis_rs::parse(&buf).expect("PDU parse failed");
    assert_eq!(pdus.len(), 1, "expected exactly one PDU after round-trip");

    let parsed = &pdus[0];

    // Header checks
    assert_eq!(
        parsed.header.exercise_id, exercise_id,
        "exercise_id mismatch"
    );
    assert_eq!(
        parsed.header.pdu_type,
        PduType::EntityState,
        "pdu_type mismatch"
    );

    // Body checks
    let body = match &parsed.body {
        PduBody::EntityState(b) => b,
        other => panic!("expected EntityState body, got {:?}", other),
    };

    // Entity ID
    assert_eq!(body.entity_id.simulation_address.site_id, state.site_id);
    assert_eq!(
        body.entity_id.simulation_address.application_id,
        state.application_id
    );
    assert_eq!(body.entity_id.entity_id, state.entity_id);

    // Location: ECEF values should survive round-trip through f64
    let (ex, ey, ez) = geodetic_to_ecef(state.latitude_deg, state.longitude_deg, state.altitude_m);
    let tol = 0.1; // 10 cm tolerance — f64 ECEF via DIS
    assert!(
        (body.entity_location.x_coordinate - ex).abs() < tol,
        "X mismatch: {} vs {}",
        body.entity_location.x_coordinate,
        ex
    );
    assert!(
        (body.entity_location.y_coordinate - ey).abs() < tol,
        "Y mismatch: {} vs {}",
        body.entity_location.y_coordinate,
        ey
    );
    assert!(
        (body.entity_location.z_coordinate - ez).abs() < tol,
        "Z mismatch: {} vs {}",
        body.entity_location.z_coordinate,
        ez
    );

    // Orientation: DIS orientation is NED Euler converted into ECEF frame.
    let tol_rad = 1e-5_f32;
    let (expected_psi, expected_theta, expected_phi) = ned_euler_to_dis_orientation(
        state.latitude_deg,
        state.longitude_deg,
        state.yaw_deg,
        state.pitch_deg,
        state.roll_deg,
    );
    assert!(
        (body.entity_orientation.psi - expected_psi).abs() < tol_rad,
        "psi mismatch"
    );
    assert!(
        (body.entity_orientation.theta - expected_theta).abs() < tol_rad,
        "theta mismatch"
    );
    assert!(
        (body.entity_orientation.phi - expected_phi).abs() < tol_rad,
        "phi mismatch"
    );

    assert_eq!(
        body.dead_reckoning_parameters.algorithm,
        DeadReckoningAlgorithm::DRM_RVW_HighSpeedOrManeuveringEntityWithExtrapolationOfOrientation,
        "flying entities must publish DRM_RVW"
    );
}

/// Header shape test: verify protocol version byte and PDU type byte are correctly positioned.
///
/// DIS v7 wire format header layout (big-endian):
///   byte 0: protocol version (7 = IEEE1278.1-2012)
///   byte 1: exercise ID
///   byte 2: PDU type (1 = EntityState)
///   byte 3: protocol family
#[test]
fn test_pdu_header_shape() {
    let state = sample_entity_state();
    let exercise_id: u8 = 10;

    let pdu = build_entity_state_pdu(&state, exercise_id, 0);
    let pdu_len = pdu.pdu_length() as usize;
    let mut buf = BytesMut::with_capacity(pdu_len);
    pdu.serialize(&mut buf).expect("serialize");

    let bytes: &[u8] = &buf;

    // Protocol version: 7 (IEEE 1278.1-2012)
    assert_eq!(bytes[0], 7, "protocol version byte should be 7 (DIS v7)");
    // Exercise ID
    assert_eq!(bytes[1], exercise_id, "exercise_id byte mismatch");
    // PDU type: 1 = EntityState
    assert_eq!(bytes[2], 1, "PDU type byte should be 1 (EntityState)");
}

/// Geodetic→ECEF conversion at well-known reference points.
#[test]
fn test_geodetic_to_ecef() {
    // Equator / prime meridian at zero altitude → (a, 0, 0)
    let a = 6_378_137.0_f64;
    let (x, y, z) = geodetic_to_ecef(0.0, 0.0, 0.0);
    assert!((x - a).abs() < 1e-3, "x at equator/prime meridian: {x}");
    assert!(y.abs() < 1e-3, "y should be zero at prime meridian: {y}");
    assert!(z.abs() < 1e-3, "z should be zero at equator: {z}");

    // Equator / 90°E at zero altitude → (0, a, 0)
    let (x2, y2, z2) = geodetic_to_ecef(0.0, 90.0, 0.0);
    assert!(x2.abs() < 1e-3, "x at 90E: {x2}");
    assert!((y2 - a).abs() < 1e-3, "y at 90E: {y2}");
    assert!(z2.abs() < 1e-3, "z at 90E: {z2}");

    // North pole → (0, 0, b) where b = a*(1 - 1/298.257223563)
    let b = a * (1.0 - 1.0 / 298.257_223_563);
    let (x3, y3, z3) = geodetic_to_ecef(90.0, 0.0, 0.0);
    assert!(x3.abs() < 1e-3, "x at north pole: {x3}");
    assert!(y3.abs() < 1e-3, "y at north pole: {y3}");
    assert!((z3 - b).abs() < 1e-3, "z at north pole: {z3}");

    // Altitude offset: at lat=0, lon=0, alt=1000 → x = a + 1000
    let (x4, y4, z4) = geodetic_to_ecef(0.0, 0.0, 1000.0);
    assert!((x4 - (a + 1000.0)).abs() < 1e-3, "x with alt=1000: {x4}");
    assert!(y4.abs() < 1e-3);
    assert!(z4.abs() < 1e-3);
}

/// NED velocity frame conversion sanity check.
#[test]
fn test_ned_to_ecef_velocity_sanity() {
    // At lat=0, lon=0: North unit vector in ECEF = (0, 0, 1)
    let (vx, vy, vz) = ned_to_ecef_velocity(0.0, 0.0, 1.0, 0.0, 0.0);
    assert!(vx.abs() < 1e-10, "vx={vx}");
    assert!(vy.abs() < 1e-10, "vy={vy}");
    assert!((vz - 1.0).abs() < 1e-10, "vz={vz}");

    // At lat=0, lon=0: East unit vector in ECEF = (0, 1, 0)
    let (vx2, vy2, vz2) = ned_to_ecef_velocity(0.0, 0.0, 0.0, 1.0, 0.0);
    assert!(vx2.abs() < 1e-10, "vx2={vx2}");
    assert!((vy2 - 1.0).abs() < 1e-10, "vy2={vy2}");
    assert!(vz2.abs() < 1e-10, "vz2={vz2}");
}

#[test]
fn test_static_entity_uses_static_dead_reckoning() {
    let mut state = sample_entity_state();
    state.is_static_entity = true;
    state.velocity_north_mps = 200.0;
    state.velocity_east_mps = 150.0;

    let pdu = build_entity_state_pdu(&state, 1, 0);
    let body = match pdu.body {
        PduBody::EntityState(body) => body,
        other => panic!("expected EntityState body, got {:?}", other),
    };

    assert_eq!(
        body.dead_reckoning_parameters.algorithm,
        DeadReckoningAlgorithm::StaticNonmovingEntity,
        "fixed entities must publish static dead reckoning regardless of velocity fields"
    );
}

#[test]
fn test_marking_sanitizes_non_ascii_and_truncates() {
    let mut state = sample_entity_state();
    state.marking = "Álpha-βeta-12345".to_string();

    let pdu = build_entity_state_pdu(&state, 1, 0);
    let body = match pdu.body {
        PduBody::EntityState(body) => body,
        other => panic!("expected EntityState body, got {:?}", other),
    };

    assert_eq!(body.entity_marking.marking_string, "lpha-eta-12");
    assert!(body.entity_marking.marking_string.is_ascii());
}

#[test]
fn test_invalid_force_id_defaults_to_other_in_direct_builder_calls() {
    let mut state = sample_entity_state();
    state.force_id = 99;

    let pdu = build_entity_state_pdu(&state, 1, 0);
    let body = match pdu.body {
        PduBody::EntityState(body) => body,
        other => panic!("expected EntityState body, got {:?}", other),
    };

    assert_eq!(
        body.force_id,
        dis_rs::enumerations::ForceId::Other,
        "direct builder calls with invalid force IDs should not silently coerce to Friendly"
    );
}

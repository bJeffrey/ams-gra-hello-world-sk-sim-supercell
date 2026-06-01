//! FlightGear protocol byte-layout contract tests.
//!
//! Verifies field offsets, packet versions, and big-endian encoding behavior.

use supercell::entity::EntityState;
use supercell::flightgear::{
    FG_NET_CTRLS_SIZE, FG_NET_CTRLS_VERSION, FG_NET_FDM_SIZE, FG_NET_FDM_VERSION, FgNetCtrls,
    FgNetFdm, read_f32_at, read_f64_at,
};

// ── Size contract ─────────────────────────────────────────────────────────────

#[test]
fn fdm_encode_size() {
    let encoded = FgNetFdm::default().encode();
    assert_eq!(
        encoded.len(),
        FG_NET_FDM_SIZE,
        "FgNetFdm encoded size must be exactly {FG_NET_FDM_SIZE} bytes"
    );
}

#[test]
fn ctrls_decode_size_and_version_zero_payload_ok() {
    // A zero payload with a valid header should decode without error.
    let mut buf = vec![0u8; FG_NET_CTRLS_SIZE];
    buf[0..4].copy_from_slice(&FG_NET_CTRLS_VERSION.to_be_bytes());

    let result = FgNetCtrls::decode(&buf);
    assert!(
        result.is_ok(),
        "decode of valid zero payload failed: {:?}",
        result.err()
    );
}

#[test]
fn ctrls_decode_short_buffer_errors() {
    let buf = vec![0u8; FG_NET_CTRLS_SIZE - 1];
    let result = FgNetCtrls::decode(&buf);
    assert!(result.is_err(), "decode of short buffer should fail");
    assert!(
        result
            .expect_err("short buffer must fail")
            .to_string()
            .contains("invalid packet size"),
        "short-buffer error must mention invalid packet size",
    );
}

#[test]
fn ctrls_decode_oversized_buffer_errors() {
    let mut buf = vec![0u8; FG_NET_CTRLS_SIZE + 4];
    buf[0..4].copy_from_slice(&FG_NET_CTRLS_VERSION.to_be_bytes());

    let result = FgNetCtrls::decode(&buf);
    assert!(result.is_err(), "decode of oversized buffer should fail");
    assert!(
        result
            .expect_err("oversized buffer must fail")
            .to_string()
            .contains("invalid packet size"),
        "oversized-buffer error must mention invalid packet size",
    );
}

#[test]
fn ctrls_decode_wrong_version_errors() {
    let mut buf = vec![0u8; FG_NET_CTRLS_SIZE];
    buf[0..4].copy_from_slice(&(FG_NET_CTRLS_VERSION + 1).to_be_bytes());

    let result = FgNetCtrls::decode(&buf);
    assert!(result.is_err(), "decode with wrong version should fail");
    assert!(
        result
            .expect_err("wrong version must fail")
            .to_string()
            .contains("unsupported version"),
        "wrong-version error must mention unsupported version",
    );
}

// ── Version field contract ─────────────────────────────────────────────────────

#[test]
fn fdm_version_field() {
    // Bytes 0..4 of the encoded FDM must be 24 in big-endian.
    let encoded = FgNetFdm::default().encode();
    let version_bytes: [u8; 4] = encoded[0..4].try_into().unwrap();
    assert_eq!(
        version_bytes,
        FG_NET_FDM_VERSION.to_be_bytes(),
        "FgNetFdm version field must be {FG_NET_FDM_VERSION} in big-endian at offset 0"
    );
}

#[test]
fn ctrls_version_field() {
    // Build a ctrls struct with default version (0), set it to 27, encode and check.
    let ctrls = FgNetCtrls {
        version: FG_NET_CTRLS_VERSION,
        ..FgNetCtrls::default()
    };
    let encoded = ctrls.encode();
    let version_bytes: [u8; 4] = encoded[0..4].try_into().unwrap();
    assert_eq!(
        version_bytes,
        FG_NET_CTRLS_VERSION.to_be_bytes(),
        "FgNetCtrls version field must be {FG_NET_CTRLS_VERSION} in big-endian at offset 0"
    );
}

// ── FDM roundtrip key fields ───────────────────────────────────────────────────

/// FgNetFdm field byte offsets (verified against the C struct layout).
///
/// Layout:
///   0  version   u32
///   4  padding   u32
///   8  longitude f64
///  16  latitude  f64
///  24  altitude  f64
///  32  agl       f32
///  36  phi       f32
///  40  theta     f32
///  44  psi       f32
const FDM_OFF_LONGITUDE: usize = 8;
const FDM_OFF_LATITUDE: usize = 16;
const FDM_OFF_ALTITUDE: usize = 24;
const FDM_OFF_PHI: usize = 36;
const FDM_OFF_THETA: usize = 40;
const FDM_OFF_PSI: usize = 44;

#[test]
fn fdm_roundtrip_key_fields() {
    let fdm = FgNetFdm {
        version: FG_NET_FDM_VERSION,
        longitude: 0.5_f64, // ~28.6 degrees, in radians
        latitude: 0.6_f64,
        altitude: 3000.0_f64,
        phi: 0.1_f32,
        theta: 0.2_f32,
        psi: 1.5_f32,
        ..FgNetFdm::default()
    };

    let encoded = fdm.encode();

    // Read back from raw bytes at known offsets — big-endian.
    let lon_raw = read_f64_at(&encoded, FDM_OFF_LONGITUDE);
    let lat_raw = read_f64_at(&encoded, FDM_OFF_LATITUDE);
    let alt_raw = read_f64_at(&encoded, FDM_OFF_ALTITUDE);
    let phi_raw = read_f32_at(&encoded, FDM_OFF_PHI);
    let theta_raw = read_f32_at(&encoded, FDM_OFF_THETA);
    let psi_raw = read_f32_at(&encoded, FDM_OFF_PSI);

    assert!(
        (lon_raw - 0.5_f64).abs() < 1e-15,
        "longitude mismatch: {lon_raw}"
    );
    assert!(
        (lat_raw - 0.6_f64).abs() < 1e-15,
        "latitude mismatch: {lat_raw}"
    );
    assert!(
        (alt_raw - 3000.0_f64).abs() < 1e-10,
        "altitude mismatch: {alt_raw}"
    );
    assert!((phi_raw - 0.1_f32).abs() < 1e-7, "phi mismatch: {phi_raw}");
    assert!(
        (theta_raw - 0.2_f32).abs() < 1e-7,
        "theta mismatch: {theta_raw}"
    );
    assert!((psi_raw - 1.5_f32).abs() < 1e-7, "psi mismatch: {psi_raw}");
}

// ── Ctrls roundtrip key fields ─────────────────────────────────────────────────

/// FgNetCtrls field byte offsets (verified against the C struct layout).
///
/// Layout:
///   0  version   u32
///   4  padding   u32
///   8  aileron   f64
///  16  elevator  f64
///  24  rudder    f64
const CTRLS_OFF_AILERON: usize = 8;
const CTRLS_OFF_ELEVATOR: usize = 16;
const CTRLS_OFF_RUDDER: usize = 24;

#[test]
fn ctrls_roundtrip_key_fields() {
    let ctrls = FgNetCtrls {
        version: FG_NET_CTRLS_VERSION,
        aileron: -0.5_f64,
        elevator: 0.3_f64,
        rudder: 0.1_f64,
        ..FgNetCtrls::default()
    };

    // Encode to bytes.
    let encoded = ctrls.encode();

    // Verify raw big-endian bytes at known offsets.
    let ail_raw = read_f64_at(&encoded, CTRLS_OFF_AILERON);
    let ele_raw = read_f64_at(&encoded, CTRLS_OFF_ELEVATOR);
    let rud_raw = read_f64_at(&encoded, CTRLS_OFF_RUDDER);

    assert!(
        (ail_raw - (-0.5_f64)).abs() < 1e-15,
        "aileron mismatch: {ail_raw}"
    );
    assert!(
        (ele_raw - 0.3_f64).abs() < 1e-15,
        "elevator mismatch: {ele_raw}"
    );
    assert!(
        (rud_raw - 0.1_f64).abs() < 1e-15,
        "rudder mismatch: {rud_raw}"
    );

    // Decode and verify struct fields match.
    let decoded = FgNetCtrls::decode(&encoded).expect("decode failed");
    assert!((decoded.aileron - (-0.5_f64)).abs() < 1e-15);
    assert!((decoded.elevator - 0.3_f64).abs() < 1e-15);
    assert!((decoded.rudder - 0.1_f64).abs() < 1e-15);
    assert_eq!(decoded.version, FG_NET_CTRLS_VERSION);
}

// ── from_entity_state unit conversion ─────────────────────────────────────────

#[test]
fn from_entity_state_converts_units() {
    let state = EntityState {
        latitude_deg: 36.0,
        longitude_deg: -115.0,
        altitude_m: 5000.0,
        altitude_msl_m: 5100.0,
        terrain_elevation_m: 4800.0,
        roll_deg: 10.0,
        pitch_deg: 5.0,
        yaw_deg: 90.0,
        velocity_north_mps: 100.0,
        velocity_east_mps: 50.0,
        velocity_down_mps: -10.0,
        ..EntityState::default()
    };

    let fdm = FgNetFdm::from_entity_state(&state);

    const DEG_TO_RAD: f64 = std::f64::consts::PI / 180.0;
    const MPS_TO_FPS: f64 = 3.280_839_895_013_123;

    // Position: degrees → radians
    assert!(
        (fdm.latitude - 36.0 * DEG_TO_RAD).abs() < 1e-12,
        "lat {}",
        fdm.latitude
    );
    assert!(
        (fdm.longitude - (-115.0) * DEG_TO_RAD).abs() < 1e-12,
        "lon {}",
        fdm.longitude
    );
    assert!((fdm.altitude - 5100.0).abs() < 1e-9, "alt {}", fdm.altitude);

    // Euler angles: degrees → radians
    assert!(
        (fdm.phi as f64 - 10.0 * DEG_TO_RAD).abs() < 1e-6,
        "phi {}",
        fdm.phi
    );
    assert!(
        (fdm.theta as f64 - 5.0 * DEG_TO_RAD).abs() < 1e-6,
        "theta {}",
        fdm.theta
    );
    assert!(
        (fdm.psi as f64 - 90.0 * DEG_TO_RAD).abs() < 1e-6,
        "psi {}",
        fdm.psi
    );

    // Velocity: m/s → fps
    assert!(
        (fdm.v_north as f64 - 100.0 * MPS_TO_FPS).abs() < 1e-3,
        "v_north {}",
        fdm.v_north
    );
    assert!(
        (fdm.v_east as f64 - 50.0 * MPS_TO_FPS).abs() < 1e-3,
        "v_east {}",
        fdm.v_east
    );

    // Version fixed at 24
    assert_eq!(fdm.version, FG_NET_FDM_VERSION);
}

// ── Encode then decode roundtrip (FDM big-endian correctness) ─────────────────

#[test]
fn fdm_encode_big_endian_version() {
    let fdm = FgNetFdm {
        version: 24,
        ..FgNetFdm::default()
    };
    let encoded = fdm.encode();
    // Version must appear as 0x00000018 at offset 0.
    assert_eq!(
        &encoded[0..4],
        &[0x00, 0x00, 0x00, 0x18],
        "version 24 must be big-endian 0x00000018"
    );
}

#[test]
fn ctrls_decode_then_reencode_roundtrip() {
    // Build a ctrls, encode it, decode it, re-encode it — bytes must be identical.
    let original = FgNetCtrls {
        version: FG_NET_CTRLS_VERSION,
        aileron: 0.75,
        elevator: -0.25,
        rudder: 0.0,
        throttle: [0.8, 0.0, 0.0, 0.0],
        freeze: 0x01,
        ..FgNetCtrls::default()
    };

    let encoded_1 = original.encode();
    let decoded = FgNetCtrls::decode(&encoded_1).expect("first decode failed");
    let encoded_2 = decoded.encode();

    assert_eq!(
        encoded_1, encoded_2,
        "encode→decode→encode must be identical bytes"
    );
}

// ── Freeze field location contract ───────────────────────────────────────────

#[test]
fn ctrls_freeze_field_roundtrip() {
    // `freeze` bit 0x01 carries the mode-cycle signal.
    // Verify decode preserves it.
    let ctrls = FgNetCtrls {
        version: FG_NET_CTRLS_VERSION,
        freeze: 0x01, // mode-cycle bit
        ..FgNetCtrls::default()
    };

    let encoded = ctrls.encode();
    let decoded = FgNetCtrls::decode(&encoded).expect("decode failed");
    assert_eq!(
        decoded.freeze, 0x01,
        "freeze field must roundtrip correctly"
    );
    assert_ne!(
        decoded.freeze & 0x01,
        0,
        "mode-cycle bit (0x01) must be set"
    );
}

//! DIS PDU construction and UDP publication.
//!
//! Use `RUST_LOG=supercell::dis=debug` to inspect publish events.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};

use anyhow::{Context, Result};
use bytes::BytesMut;
use dis_rs::entity_state::builder::EntityStateBuilder;
use dis_rs::entity_state::model::{DrParameters, EntityAppearance, EntityMarking};
use dis_rs::enumerations::{
    AirPlatformAppearance, Country, DeadReckoningAlgorithm, EntityKind as DisEntityKindEnum,
    ForceId, PduType, PlatformDomain,
};
use dis_rs::model::{
    EntityId, EntityType, Location, Orientation, Pdu, PduBody, PduHeader, VectorF32,
};
use socket2::{Domain, Protocol, Socket, Type};
use time::OffsetDateTime;
use tracing::debug;

use crate::config::DisConfig;
use crate::entity::EntityState;

// ─── Coordinate conversion ────────────────────────────────────────────────────

/// Convert geodetic (lat, lon, alt) to ECEF (x, y, z).
///
/// * `lat_deg` — geodetic latitude in degrees (positive North)
/// * `lon_deg` — geodetic longitude in degrees (positive East)
/// * `alt_m`   — altitude above WGS-84 ellipsoid in metres
///
/// Returns `(x_m, y_m, z_m)` in Earth-Centred Earth-Fixed frame.
pub fn geodetic_to_ecef(lat_deg: f64, lon_deg: f64, alt_m: f64) -> (f64, f64, f64) {
    map_3d::geodetic2ecef(
        lat_deg.to_radians(),
        lon_deg.to_radians(),
        alt_m,
        map_3d::Ellipsoid::default(),
    )
}

/// Convert NED (North, East, Down) velocity to ECEF velocity components.
///
/// The NED frame is centred at the geodetic position given by `lat_deg` / `lon_deg`.
pub fn ned_to_ecef_velocity(
    lat_deg: f64,
    lon_deg: f64,
    v_north: f64,
    v_east: f64,
    v_down: f64,
) -> (f64, f64, f64) {
    let lat = lat_deg.to_radians();
    let lon = lon_deg.to_radians();

    let sin_lat = lat.sin();
    let cos_lat = lat.cos();
    let sin_lon = lon.sin();
    let cos_lon = lon.cos();

    // Rotation matrix from NED to ECEF (columns are unit vectors of NED axes in ECEF)
    //  North:  (-sin_lat*cos_lon, -sin_lat*sin_lon,  cos_lat)
    //  East:   (-sin_lon,          cos_lon,           0)
    //  Down:   (-cos_lat*cos_lon, -cos_lat*sin_lon,  -sin_lat)
    let vx = -sin_lat * cos_lon * v_north - sin_lon * v_east - cos_lat * cos_lon * v_down;
    let vy = -sin_lat * sin_lon * v_north + cos_lon * v_east - cos_lat * sin_lon * v_down;
    let vz = cos_lat * v_north - sin_lat * v_down;

    (vx, vy, vz)
}

/// Convert NED Euler angles (heading, pitch, roll) to DIS ECEF Euler angles (psi, theta, phi).
///
/// DIS IEEE 1278.1 defines entity orientation as three successive rotations
/// (psi, theta, phi) that transform from the ECEF reference frame to the
/// entity body frame.  JSBSim provides heading/pitch/roll in the local NED
/// frame, so we must compose the NED→body rotation with the ECEF→NED rotation.
///
/// The ECEF→NED rotation depends on the entity's geodetic position (lat, lon).
/// The full transformation builds the 3×3 rotation matrix R_ecef_to_body =
/// R_ned_to_body × R_ecef_to_ned, then extracts DIS Euler angles from it.
///
/// Inputs (all in degrees):
///   - `lat_deg`, `lon_deg`: entity geodetic position
///   - `heading_deg`: true heading (0=N, 90=E, clockwise)
///   - `pitch_deg`: nose up positive
///   - `roll_deg`: right wing down positive
///
/// Returns `(psi, theta, phi)` in radians — DIS ECEF Euler angles.
pub fn ned_euler_to_dis_orientation(
    lat_deg: f64,
    lon_deg: f64,
    heading_deg: f64,
    pitch_deg: f64,
    roll_deg: f64,
) -> (f32, f32, f32) {
    let lat = lat_deg.to_radians();
    let lon = lon_deg.to_radians();
    let hdg = heading_deg.to_radians();
    let pitch = pitch_deg.to_radians();
    let roll = roll_deg.to_radians();

    // R_ecef_to_ned: rotation matrix from ECEF to NED at (lat, lon)
    // This is the transpose of the NED-to-ECEF matrix used for velocity.
    //
    //  Row 0 (North): (-sin_lat*cos_lon, -sin_lat*sin_lon,  cos_lat)
    //  Row 1 (East):  (-sin_lon,          cos_lon,           0      )
    //  Row 2 (Down):  (-cos_lat*cos_lon, -cos_lat*sin_lon, -sin_lat )
    let sin_lat = lat.sin();
    let cos_lat = lat.cos();
    let sin_lon = lon.sin();
    let cos_lon = lon.cos();

    let r_en = [
        [-sin_lat * cos_lon, -sin_lat * sin_lon, cos_lat],
        [-sin_lon, cos_lon, 0.0],
        [-cos_lat * cos_lon, -cos_lat * sin_lon, -sin_lat],
    ];

    // R_ned_to_body: standard aerospace Euler rotation (heading, pitch, roll)
    //   = R_roll(phi) × R_pitch(theta) × R_yaw(psi)
    let sh = hdg.sin();
    let ch = hdg.cos();
    let sp = pitch.sin();
    let cp = pitch.cos();
    let sr = roll.sin();
    let cr = roll.cos();

    let r_nb = [
        [ch * cp, sh * cp, -sp],
        [ch * sp * sr - sh * cr, sh * sp * sr + ch * cr, cp * sr],
        [ch * sp * cr + sh * sr, sh * sp * cr - ch * sr, cp * cr],
    ];

    // R_ecef_to_body = R_ned_to_body × R_ecef_to_ned
    let mut r_eb = [[0.0f64; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            r_eb[i][j] =
                r_nb[i][0] * r_en[0][j] + r_nb[i][1] * r_en[1][j] + r_nb[i][2] * r_en[2][j];
        }
    }

    // Extract DIS Euler angles from R_ecef_to_body:
    //   theta_dis = -asin(R[0][2])
    //   psi_dis   = atan2(R[0][1], R[0][0])
    //   phi_dis   = atan2(R[1][2], R[2][2])
    let theta_dis = (-r_eb[0][2]).clamp(-1.0, 1.0).asin();
    let psi_dis = r_eb[0][1].atan2(r_eb[0][0]);
    let phi_dis = r_eb[1][2].atan2(r_eb[2][2]);

    (psi_dis as f32, theta_dis as f32, phi_dis as f32)
}

// ─── PDU builder ─────────────────────────────────────────────────────────────

/// Convert a free-form marking string into a DIS-safe ASCII marking.
///
/// DIS Entity Marking uses 11 ASCII characters. Non-ASCII characters are
/// removed, then the remaining ASCII content is truncated to 11 characters.
fn sanitize_dis_marking(marking: &str) -> String {
    marking.chars().filter(char::is_ascii).take(11).collect()
}

/// Build a dis-rs `Pdu` (EntityState body + v7 header) from a SuperCell `EntityState`.
///
/// Extracted into a free function so contract tests can call it without a live socket.
pub fn build_entity_state_pdu(state: &EntityState, exercise_id: u8, timestamp: u32) -> Pdu {
    // Convert position
    let (x, y, z) = geodetic_to_ecef(state.latitude_deg, state.longitude_deg, state.altitude_m);

    // Convert velocity from NED to ECEF
    let (vx, vy, vz) = ned_to_ecef_velocity(
        state.latitude_deg,
        state.longitude_deg,
        state.velocity_north_mps,
        state.velocity_east_mps,
        state.velocity_down_mps,
    );

    // DIS orientation: convert NED Euler angles to ECEF frame.
    let (psi, theta, phi) = ned_euler_to_dis_orientation(
        state.latitude_deg,
        state.longitude_deg,
        state.yaw_deg,
        state.pitch_deg,
        state.roll_deg,
    );

    // `force_id` values are validated at scenario load in `main`; this branch
    // keeps a defensive fallback for non-launch call paths such as direct tests.
    let force_id = match state.force_id {
        1 => ForceId::Friendly,
        2 => ForceId::Opposing,
        3 => ForceId::Neutral,
        _ => ForceId::Other,
    };

    let entity_id = EntityId::new(state.site_id, state.application_id, state.entity_id);

    let et = &state.entity_type;
    let entity_type = EntityType::default()
        .with_kind(DisEntityKindEnum::from(et.kind))
        .with_domain(PlatformDomain::from(et.domain))
        .with_country(Country::from(et.country))
        .with_category(et.category)
        .with_subcategory(et.subcategory)
        .with_specific(et.specific)
        .with_extra(et.extra);

    // Marking: DIS-safe ASCII with 11-character hard limit.
    let marking = EntityMarking::new_ascii(sanitize_dis_marking(&state.marking));

    // Dead Reckoning algorithm selection:
    // - Statically configured entities (Fixed): Static (1).
    // - Flying/runtime-stepped entities: DRM_RVW (4).
    let is_static = state.is_static_entity;

    let dr_params = if is_static {
        DrParameters::default().with_algorithm(DeadReckoningAlgorithm::StaticNonmovingEntity)
    } else {
        DrParameters::default()
            .with_algorithm(DeadReckoningAlgorithm::DRM_RVW_HighSpeedOrManeuveringEntityWithExtrapolationOfOrientation)
            .with_linear_acceleration(VectorF32::new(
                state.accel_x,
                state.accel_y,
                state.accel_z,
            ))
            .with_angular_velocity(VectorF32::new(
                state.roll_rate_rps as f32,
                state.pitch_rate_rps as f32,
                state.yaw_rate_rps as f32,
            ))
    };

    // Entity appearance — use AirPlatformAppearance for flying entities.
    // We use `is_frozen` (bit 21) to signal "manual override active".
    // Semantically: frozen = simulation not driving the entity = pilot has control.
    let appearance = EntityAppearance::AirPlatform(AirPlatformAppearance {
        power_plant_on: !is_static,
        is_frozen: state.manual_override, // AP override → pilot control
        ..Default::default()
    });

    let body = EntityStateBuilder::new()
        .with_entity_id(entity_id)
        .with_entity_type(entity_type)
        .with_force_id(force_id)
        .with_appearance(appearance)
        .with_location(Location::new(x, y, z))
        .with_orientation(Orientation::new(psi, theta, phi))
        .with_velocity(VectorF32::new(vx as f32, vy as f32, vz as f32))
        .with_dead_reckoning_parameters(dr_params)
        .with_marking(marking)
        .build();

    // Finalization derives the protocol family and mandatory wire length from
    // the body. Constructing `Pdu` directly leaves the serialized length zero,
    // which strict DIS consumers correctly reject.
    Pdu::finalize_from_parts(
        PduHeader::new_v7(exercise_id, PduType::EntityState),
        PduBody::EntityState(body),
        timestamp,
    )
}

// ─── Publisher ────────────────────────────────────────────────────────────────

/// Convert a duration since UNIX epoch into an absolute DIS timestamp.
///
/// DIS timestamps represent the time past the hour in units of `(3600 / (2^31 - 1))` seconds.
/// Absolute time is indicated by setting the least significant bit (LSB) to 1.
pub fn dis_timestamp_from_duration(since_epoch: std::time::Duration) -> u32 {
    // Calculate seconds past the top of the hour.
    #[allow(clippy::cast_precision_loss)]
    let seconds_past_hour =
        (since_epoch.as_secs() % 3600) as f64 + (f64::from(since_epoch.subsec_nanos()) / 1e9);
    // Convert to DIS timestamp units: 3600 seconds = 2^31 - 1 units.
    #[allow(clippy::cast_sign_loss)]
    let units = (seconds_past_hour * (2_147_483_647.0 / 3600.0)).round() as u32;
    // Shift left by 1 and set the LSB to 1 to indicate absolute time.
    (units << 1) | 1
}

/// Return the current absolute DIS timestamp.
///
/// DIS timestamps represent the time past the hour in units of `(3600 / (2^31 - 1))` seconds.
/// Absolute time is indicated by setting the least significant bit (LSB) to 1.
pub fn current_dis_timestamp() -> u32 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    dis_timestamp_from_duration(now)
}

/// Convert an authoritative UTC scenario timestamp into an absolute DIS timestamp.
pub fn dis_timestamp_from_datetime(timestamp: OffsetDateTime) -> u32 {
    let nanos = timestamp.unix_timestamp_nanos().max(0);
    let seconds = u64::try_from(nanos / 1_000_000_000).unwrap_or(u64::MAX);
    let subsec_nanos = u32::try_from(nanos % 1_000_000_000).unwrap_or_default();
    dis_timestamp_from_duration(std::time::Duration::new(seconds, subsec_nanos))
}

/// Publishes DIS EntityStatePdu packets over a UDP multicast socket.
pub struct DisPublisher {
    socket: UdpSocket,
    target: SocketAddr,
    exercise_id: u8,
    buf: BytesMut,
}

impl DisPublisher {
    /// Bind an ephemeral UDP socket configured for DIS sending and return
    /// a ready-to-use publisher.
    ///
    /// When `multicast_addr` is a true multicast address (224.0.0.0/4) the
    /// socket is configured with `IP_MULTICAST_TTL`, `IP_MULTICAST_LOOP`,
    /// and `IP_MULTICAST_IF`.  When it is a unicast address (e.g. `127.0.0.1`
    /// for loopback testing on hosts where multicast loopback is broken) those
    /// options are skipped. In all cases without an explicitly configured
    /// interface, the socket binds to the unspecified address (0.0.0.0) on an
    /// ephemeral port to let the OS determine the correct default-route interface.
    pub fn new(config: &DisConfig) -> Result<Self> {
        let target_addr: Ipv4Addr = config
            .multicast_addr
            .parse()
            .context("parse multicast_addr")?;

        let is_multicast = target_addr.octets()[0] >= 224 && target_addr.octets()[0] <= 239;

        // Build socket with SO_REUSEADDR via socket2
        let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))
            .context("create UDP socket")?;
        sock.set_reuse_address(true).context("SO_REUSEADDR")?;

        // Resolve the outgoing interface / bind address.
        //
        // Priority order:
        //   1. Explicit `multicast_iface` from config.
        //   2. For unicast targets: Ipv4Addr::UNSPECIFIED (0.0.0.0) — let the OS
        //      pick the default-route interface based on the target address.
        //   3. For multicast targets: Ipv4Addr::UNSPECIFIED (0.0.0.0) — let the OS
        //      pick the default-route interface.
        let iface_addr: Ipv4Addr = config
            .multicast_iface
            .as_deref()
            .map(|s| s.parse().context("parse multicast_iface"))
            .transpose()?
            .unwrap_or(Ipv4Addr::UNSPECIFIED);

        if is_multicast {
            let ttl = config.ttl.unwrap_or(1);
            sock.set_multicast_ttl_v4(ttl).context("IP_MULTICAST_TTL")?;

            // Enable multicast loopback so that other sockets on the same host
            // (including containerized senders with --network host) can receive
            // the datagrams.  This is the standard DIS simulator behaviour.
            sock.set_multicast_loop_v4(true)
                .context("IP_MULTICAST_LOOP")?;

            sock.set_multicast_if_v4(&iface_addr)
                .context("IP_MULTICAST_IF")?;
        }

        // Bind to the chosen interface address on an ephemeral port (send-only).
        let bind_addr: SocketAddr = SocketAddr::new(IpAddr::V4(iface_addr), 0);
        sock.bind(&bind_addr.into()).context("bind UDP socket")?;

        let socket: UdpSocket = sock.into();

        let target = SocketAddr::new(IpAddr::V4(target_addr), config.port);

        let exercise_id = config.exercise_id;

        Ok(Self {
            socket,
            target,
            exercise_id,
            buf: BytesMut::with_capacity(1024),
        })
    }

    /// Serialize `state` as an EntityStatePdu and send it to the multicast target.
    pub fn publish(&mut self, state: &EntityState) -> Result<()> {
        self.publish_at(state, OffsetDateTime::now_utc())
    }

    /// Serialize `state` with its authoritative scenario timestamp and send it.
    pub fn publish_at(&mut self, state: &EntityState, scenario_time: OffsetDateTime) -> Result<()> {
        let timestamp = dis_timestamp_from_datetime(scenario_time);

        let pdu = build_entity_state_pdu(state, self.exercise_id, timestamp);

        // Resize buffer to exactly the required capacity
        let pdu_len = pdu.pdu_length() as usize;
        self.buf.clear();
        if self.buf.capacity() < pdu_len {
            self.buf.reserve(pdu_len - self.buf.capacity());
        }

        pdu.serialize(&mut self.buf)
            .map_err(|e| anyhow::anyhow!("PDU serialize error: {:?}", e))?;

        let bytes_sent = self
            .socket
            .send_to(&self.buf, self.target)
            .context("send_to multicast")?;

        debug!(
            entity_id = state.entity_id,
            site_id = state.site_id,
            application_id = state.application_id,
            bytes = bytes_sent,
            "dis.publish"
        );
        Ok(())
    }
}

// ─── Unit tests (coordinate conversion) ──────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const TOLERANCE: f64 = 1e-3; // 1 mm tolerance
    const WGS84_A: f64 = 6_378_137.0;
    const WGS84_F: f64 = 1.0 / 298.257_223_563;

    #[test]
    fn test_geodetic_to_ecef_equator_prime_meridian() {
        // At lat=0, lon=0, alt=0 we should be at (a, 0, 0)
        let (x, y, z) = geodetic_to_ecef(0.0, 0.0, 0.0);
        assert!((x - WGS84_A).abs() < TOLERANCE, "x={x}, expected {WGS84_A}");
        assert!(y.abs() < TOLERANCE, "y={y}, expected 0");
        assert!(z.abs() < TOLERANCE, "z={z}, expected 0");
    }

    #[test]
    fn test_geodetic_to_ecef_north_pole() {
        // At lat=90, lon=0, alt=0 we should be at (0, 0, b) where b = a*(1-f)
        let b = WGS84_A * (1.0 - WGS84_F);
        let (x, y, z) = geodetic_to_ecef(90.0, 0.0, 0.0);
        assert!(x.abs() < TOLERANCE, "x={x}, expected 0");
        assert!(y.abs() < TOLERANCE, "y={y}, expected 0");
        assert!((z - b).abs() < TOLERANCE, "z={z}, expected b={b}");
    }

    #[test]
    fn test_geodetic_to_ecef_altitude_offset() {
        // At lat=0, lon=0, alt=1000 we expect x = a + 1000
        let (x, y, z) = geodetic_to_ecef(0.0, 0.0, 1000.0);
        assert!((x - (WGS84_A + 1000.0)).abs() < TOLERANCE);
        assert!(y.abs() < TOLERANCE);
        assert!(z.abs() < TOLERANCE);
    }

    #[test]
    fn test_ned_to_ecef_north_velocity_at_origin() {
        // At lat=0, lon=0: pure North velocity should map to +z in ECEF
        // North unit vector at (lat=0,lon=0): (-sin(0)*cos(0), -sin(0)*sin(0), cos(0)) = (0,0,1)
        let (vx, vy, vz) = ned_to_ecef_velocity(0.0, 0.0, 1.0, 0.0, 0.0);
        assert!(vx.abs() < 1e-10, "vx={vx}");
        assert!(vy.abs() < 1e-10, "vy={vy}");
        assert!((vz - 1.0).abs() < 1e-10, "vz={vz}");
    }

    #[test]
    fn test_sanitize_dis_marking_ascii_and_length() {
        let marking = sanitize_dis_marking("Álpha-βeta-12345");
        assert_eq!(marking, "lpha-eta-12");
        assert!(marking.is_ascii());
        assert_eq!(marking.chars().count(), 11);
    }

    #[test]
    fn test_current_dis_timestamp_is_absolute() {
        let ts = current_dis_timestamp();
        assert_eq!(ts & 1, 1, "LSB must be 1 for absolute timestamps");
    }

    #[test]
    fn test_dis_timestamp_boundaries() {
        use std::time::Duration;

        // Top of the hour
        let ts_0 = dis_timestamp_from_duration(Duration::from_secs(0));
        assert_eq!(ts_0, 1, "0 seconds should be 0 units, LSB=1 -> 1");

        // Next hour wraps around exactly
        let ts_hour = dis_timestamp_from_duration(Duration::from_secs(3600));
        assert_eq!(
            ts_hour, 1,
            "3600 seconds should wrap to 0 units, LSB=1 -> 1"
        );

        // Half hour
        let ts_half = dis_timestamp_from_duration(Duration::from_secs(1800));
        // 1800 is exactly half, so units = 2147483647 / 2 = 1073741823.5 -> 1073741824
        // Shift left + 1 = 2147483649
        assert_eq!(ts_half, (1_073_741_824 << 1) | 1);

        // Just before rollover (max possible value)
        let ts_max = dis_timestamp_from_duration(
            Duration::from_secs(3599) + Duration::from_nanos(999_999_999),
        );
        // This evaluates exactly to u32::MAX. Since it's < 3600, it stays within limits.
        assert_eq!(
            ts_max, 4_294_967_295,
            "max nanos should evaluate to u32::MAX without overflowing"
        );
    }

    #[test]
    fn test_ned_euler_to_dis_orientation_precision_bounds() {
        // Construct a scenario that evaluates to asin(> 1.0) without clamping
        // R_en[0][2] = cos(lat)
        // R_nb[0][0] = cos(hdg)*cos(pitch)
        // R_nb[0][2] = -sin(pitch)
        // Let's just force a combination that mathematically yields exactly 1.0 or -1.0,
        // and due to floating point error could exceed it.
        // For example, lat=8, lon=0, heading=0, pitch=8, roll=0 yields ~ 1.0
        // theta_dis = -asin(r_eb[0][2])
        // With precision error, r_eb[0][2] can become slightly < -1.0 or > 1.0.
        // We just ensure it doesn't return NaN for extreme values.
        let (psi, theta, phi) = ned_euler_to_dis_orientation(8.0, 0.0, 0.0, 8.0, 0.0);
        assert!(!psi.is_nan());
        assert!(!theta.is_nan());
        assert!(!phi.is_nan());

        let (psi, theta, phi) = ned_euler_to_dis_orientation(8.0, 0.0, 180.0, -8.0, 0.0);
        assert!(!psi.is_nan());
        assert!(!theta.is_nan());
        assert!(!phi.is_nan());
    }
}

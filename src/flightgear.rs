//! FlightGear protocol structs and UDP bridge.
//!
//! Implements `FGNetFDM` (version 24) and `FGNetCtrls` (version 27) with big-endian wire encoding.

use std::net::UdpSocket;

use anyhow::{Context, Result, bail};
use tracing::{debug, error, warn};

use crate::entity::EntityState;

// ── Constants ─────────────────────────────────────────────────────────────────

/// FlightGear `FGNetFDM` protocol version (matches `net_fdm.hxx`).
pub const FG_NET_FDM_VERSION: u32 = 24;
/// FlightGear `FGNetCtrls` protocol version (matches `net_ctrls.hxx`).
pub const FG_NET_CTRLS_VERSION: u32 = 27;

const FG_MAX_ENGINES_FDM: usize = 4;
const FG_MAX_WHEELS_FDM: usize = 3;
const FG_MAX_TANKS_FDM: usize = 4;

const FG_MAX_ENGINES_CTRLS: usize = 4;
const FG_MAX_TANKS_CTRLS: usize = 8;
const RESERVED_SPACE: usize = 25;

/// Expected encoded size of [`FgNetFdm`] in bytes (matches sizeof(FGNetFDM) in C).
pub const FG_NET_FDM_SIZE: usize = 408;
/// Expected encoded size of [`FgNetCtrls`] in bytes (matches sizeof(FGNetCtrls) in C).
pub const FG_NET_CTRLS_SIZE: usize = 744;

// ── FgNetFdm ──────────────────────────────────────────────────────────────────

/// Flight-dynamics state packet sent **to** FlightGear.
///
/// Matches the field order and types of `FGNetFDM` in `net_fdm.hxx` (version 24).
/// All fields use SI units **except** velocities, which FlightGear expects in
/// feet-per-second, and angles, which must be in radians.
#[derive(Debug, Clone)]
pub struct FgNetFdm {
    // Header
    /// Protocol version (must be 24).
    pub version: u32,
    /// Struct padding for alignment.
    pub padding: u32,

    // Positions
    /// Geodetic longitude in radians.
    pub longitude: f64,
    /// Geodetic latitude in radians.
    pub latitude: f64,
    /// Altitude above sea level in metres.
    pub altitude: f64,
    /// Altitude above ground level in metres.
    pub agl: f32,
    /// Roll angle in radians.
    pub phi: f32,
    /// Pitch angle in radians.
    pub theta: f32,
    /// Yaw / true heading in radians.
    pub psi: f32,
    /// Angle of attack in radians.
    pub alpha: f32,
    /// Side-slip angle in radians.
    pub beta: f32,

    // Velocities
    /// Roll rate in rad/s.
    pub phidot: f32,
    /// Pitch rate in rad/s.
    pub thetadot: f32,
    /// Yaw rate in rad/s.
    pub psidot: f32,
    /// Calibrated airspeed in knots.
    pub vcas: f32,
    /// Climb rate in ft/s.
    pub climb_rate: f32,
    /// North velocity in ft/s.
    pub v_north: f32,
    /// East velocity in ft/s.
    pub v_east: f32,
    /// Down velocity in ft/s.
    pub v_down: f32,
    /// Body-frame forward velocity in ft/s.
    pub v_body_u: f32,
    /// Body-frame right velocity in ft/s.
    pub v_body_v: f32,
    /// Body-frame down velocity in ft/s.
    pub v_body_w: f32,

    // Accelerations
    /// Pilot X-axis acceleration in ft/s².
    pub a_x_pilot: f32,
    /// Pilot Y-axis acceleration in ft/s².
    pub a_y_pilot: f32,
    /// Pilot Z-axis acceleration in ft/s².
    pub a_z_pilot: f32,

    // Stall
    /// Stall warning indicator (0.0–1.0).
    pub stall_warning: f32,
    /// Sideslip angle in degrees.
    pub slip_deg: f32,

    // Engine status
    /// Number of engines.
    pub num_engines: u32,
    /// Engine state per engine (0=off, 2=running).
    pub eng_state: [u32; FG_MAX_ENGINES_FDM],
    /// Engine RPM per engine.
    pub rpm: [f32; FG_MAX_ENGINES_FDM],
    /// Fuel flow per engine.
    pub fuel_flow: [f32; FG_MAX_ENGINES_FDM],
    /// Fuel pressure per engine.
    pub fuel_px: [f32; FG_MAX_ENGINES_FDM],
    /// Exhaust gas temperature per engine (°F).
    pub egt: [f32; FG_MAX_ENGINES_FDM],
    /// Cylinder head temperature per engine (°F).
    pub cht: [f32; FG_MAX_ENGINES_FDM],
    /// Manifold pressure per engine (inHg).
    pub mp_osi: [f32; FG_MAX_ENGINES_FDM],
    /// Turbine inlet temperature per engine.
    pub tit: [f32; FG_MAX_ENGINES_FDM],
    /// Oil temperature per engine (°F).
    pub oil_temp: [f32; FG_MAX_ENGINES_FDM],
    /// Oil pressure per engine (PSI).
    pub oil_px: [f32; FG_MAX_ENGINES_FDM],

    // Consumables
    /// Number of fuel tanks.
    pub num_tanks: u32,
    /// Fuel quantity per tank (gallons).
    pub fuel_quantity: [f32; FG_MAX_TANKS_FDM],

    // Gear status
    /// Number of landing gear wheels.
    pub num_wheels: u32,
    /// Weight-on-wheels flag per wheel.
    pub wow: [u32; FG_MAX_WHEELS_FDM],
    /// Gear position per wheel (0=retracted, 1=extended).
    pub gear_pos: [f32; FG_MAX_WHEELS_FDM],
    /// Gear steering angle per wheel.
    pub gear_steer: [f32; FG_MAX_WHEELS_FDM],
    /// Gear compression per wheel.
    pub gear_compression: [f32; FG_MAX_WHEELS_FDM],

    // Environment
    /// Current Unix time in seconds.
    pub cur_time: u32,
    /// Time warp offset in seconds.
    pub warp: i32,
    /// Visibility in metres.
    pub visibility: f32,

    // Control surface positions (normalized)
    /// Elevator position normalized.
    pub elevator: f32,
    /// Elevator trim tab position normalized.
    pub elevator_trim_tab: f32,
    /// Left flap position normalized.
    pub left_flap: f32,
    /// Right flap position normalized.
    pub right_flap: f32,
    /// Left aileron position normalized.
    pub left_aileron: f32,
    /// Right aileron position normalized.
    pub right_aileron: f32,
    /// Rudder position normalized.
    pub rudder: f32,
    /// Nose wheel steering angle.
    pub nose_wheel: f32,
    /// Speedbrake position normalized.
    pub speedbrake: f32,
    /// Spoilers position normalized.
    pub spoilers: f32,
}

impl Default for FgNetFdm {
    fn default() -> Self {
        Self {
            version: FG_NET_FDM_VERSION,
            padding: 0,
            longitude: 0.0,
            latitude: 0.0,
            altitude: 0.0,
            agl: 0.0,
            phi: 0.0,
            theta: 0.0,
            psi: 0.0,
            alpha: 0.0,
            beta: 0.0,
            phidot: 0.0,
            thetadot: 0.0,
            psidot: 0.0,
            vcas: 0.0,
            climb_rate: 0.0,
            v_north: 0.0,
            v_east: 0.0,
            v_down: 0.0,
            v_body_u: 0.0,
            v_body_v: 0.0,
            v_body_w: 0.0,
            a_x_pilot: 0.0,
            a_y_pilot: 0.0,
            a_z_pilot: 0.0,
            stall_warning: 0.0,
            slip_deg: 0.0,
            num_engines: 0,
            eng_state: [0; FG_MAX_ENGINES_FDM],
            rpm: [0.0; FG_MAX_ENGINES_FDM],
            fuel_flow: [0.0; FG_MAX_ENGINES_FDM],
            fuel_px: [0.0; FG_MAX_ENGINES_FDM],
            egt: [0.0; FG_MAX_ENGINES_FDM],
            cht: [0.0; FG_MAX_ENGINES_FDM],
            mp_osi: [0.0; FG_MAX_ENGINES_FDM],
            tit: [0.0; FG_MAX_ENGINES_FDM],
            oil_temp: [0.0; FG_MAX_ENGINES_FDM],
            oil_px: [0.0; FG_MAX_ENGINES_FDM],
            num_tanks: 0,
            fuel_quantity: [0.0; FG_MAX_TANKS_FDM],
            num_wheels: 0,
            wow: [0; FG_MAX_WHEELS_FDM],
            gear_pos: [0.0; FG_MAX_WHEELS_FDM],
            gear_steer: [0.0; FG_MAX_WHEELS_FDM],
            gear_compression: [0.0; FG_MAX_WHEELS_FDM],
            cur_time: 0,
            warp: 0,
            visibility: 0.0,
            elevator: 0.0,
            elevator_trim_tab: 0.0,
            left_flap: 0.0,
            right_flap: 0.0,
            left_aileron: 0.0,
            right_aileron: 0.0,
            rudder: 0.0,
            nose_wheel: 0.0,
            speedbrake: 0.0,
            spoilers: 0.0,
        }
    }
}

impl FgNetFdm {
    /// Construct an `FgNetFdm` from an [`EntityState`], converting units as required.
    ///
    /// - Lat/lon degrees → radians
    /// - Euler angles degrees → radians
    /// - NED velocity m/s → fps (× 3.28084)
    /// - version set to 24; engine/gear/env fields zeroed
    pub fn from_entity_state(state: &EntityState) -> Self {
        const DEG_TO_RAD: f64 = std::f64::consts::PI / 180.0;
        const MPS_TO_FPS: f64 = 3.280_839_895_013_123;

        let agl_m = (state.altitude_msl_m - state.terrain_elevation_m).max(0.0);

        Self {
            version: FG_NET_FDM_VERSION,
            padding: 0,
            longitude: state.longitude_deg * DEG_TO_RAD,
            latitude: state.latitude_deg * DEG_TO_RAD,
            altitude: state.altitude_msl_m,
            agl: agl_m as f32,
            phi: (state.roll_deg * DEG_TO_RAD) as f32,
            theta: (state.pitch_deg * DEG_TO_RAD) as f32,
            psi: (state.yaw_deg * DEG_TO_RAD) as f32,
            alpha: (state.alpha_deg as f64 * DEG_TO_RAD) as f32,
            beta: (state.beta_deg as f64 * DEG_TO_RAD) as f32,
            phidot: state.roll_rate_rps as f32,
            thetadot: state.pitch_rate_rps as f32,
            psidot: state.yaw_rate_rps as f32,
            vcas: state.vcas_kts,
            climb_rate: (-state.velocity_down_mps * MPS_TO_FPS) as f32,
            v_north: (state.velocity_north_mps * MPS_TO_FPS) as f32,
            v_east: (state.velocity_east_mps * MPS_TO_FPS) as f32,
            v_down: (state.velocity_down_mps * MPS_TO_FPS) as f32,
            v_body_u: state.v_body_u_fps,
            v_body_v: state.v_body_v_fps,
            v_body_w: state.v_body_w_fps,
            a_x_pilot: state.a_x_pilot_fpss,
            a_y_pilot: state.a_y_pilot_fpss,
            a_z_pilot: state.a_z_pilot_fpss,
            stall_warning: state.stall_warning,
            slip_deg: 0.0,
            num_engines: 1,
            eng_state: [2, 0, 0, 0], // 2 = running
            rpm: [state.engine_rpm, 0.0, 0.0, 0.0],
            fuel_flow: [state.engine_fuel_flow_gph, 0.0, 0.0, 0.0],
            fuel_px: [0.0; FG_MAX_ENGINES_FDM],
            egt: [state.engine_egt_degf, 0.0, 0.0, 0.0],
            cht: [state.engine_cht_degf, 0.0, 0.0, 0.0],
            mp_osi: [state.engine_mp_inhg, 0.0, 0.0, 0.0],
            tit: [0.0; FG_MAX_ENGINES_FDM],
            oil_temp: [state.engine_oil_temp_degf, 0.0, 0.0, 0.0],
            oil_px: [state.engine_oil_press_psi, 0.0, 0.0, 0.0],
            num_tanks: 2,
            fuel_quantity: [26.0, 26.0, 0.0, 0.0], // 26 gal per tank (C-172 full)
            num_wheels: 3,
            wow: [0; FG_MAX_WHEELS_FDM],
            gear_pos: [
                state.gear_pos_norm,
                state.gear_pos_norm,
                state.gear_pos_norm,
            ],
            gear_steer: [0.0; FG_MAX_WHEELS_FDM],
            gear_compression: [0.0; FG_MAX_WHEELS_FDM],
            cur_time: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as u32,
            warp: 0,
            visibility: 10_000.0,
            elevator: state.elevator_pos_norm,
            elevator_trim_tab: state.elevator_trim_norm,
            left_flap: state.flap_pos_norm,
            right_flap: state.flap_pos_norm,
            left_aileron: state.left_aileron_pos_norm,
            right_aileron: state.right_aileron_pos_norm,
            rudder: state.rudder_pos_norm,
            nose_wheel: 0.0,
            speedbrake: 0.0,
            spoilers: 0.0,
        }
    }

    /// Encode to a big-endian byte buffer in the same field order as the C struct.
    ///
    /// The resulting `Vec<u8>` will always be exactly [`FG_NET_FDM_SIZE`] bytes.
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(FG_NET_FDM_SIZE);

        // Header
        buf.extend_from_slice(&self.version.to_be_bytes());
        buf.extend_from_slice(&self.padding.to_be_bytes());

        // Positions
        buf.extend_from_slice(&self.longitude.to_bits().to_be_bytes());
        buf.extend_from_slice(&self.latitude.to_bits().to_be_bytes());
        buf.extend_from_slice(&self.altitude.to_bits().to_be_bytes());
        buf.extend_from_slice(&self.agl.to_bits().to_be_bytes());
        buf.extend_from_slice(&self.phi.to_bits().to_be_bytes());
        buf.extend_from_slice(&self.theta.to_bits().to_be_bytes());
        buf.extend_from_slice(&self.psi.to_bits().to_be_bytes());
        buf.extend_from_slice(&self.alpha.to_bits().to_be_bytes());
        buf.extend_from_slice(&self.beta.to_bits().to_be_bytes());

        // Velocities
        buf.extend_from_slice(&self.phidot.to_bits().to_be_bytes());
        buf.extend_from_slice(&self.thetadot.to_bits().to_be_bytes());
        buf.extend_from_slice(&self.psidot.to_bits().to_be_bytes());
        buf.extend_from_slice(&self.vcas.to_bits().to_be_bytes());
        buf.extend_from_slice(&self.climb_rate.to_bits().to_be_bytes());
        buf.extend_from_slice(&self.v_north.to_bits().to_be_bytes());
        buf.extend_from_slice(&self.v_east.to_bits().to_be_bytes());
        buf.extend_from_slice(&self.v_down.to_bits().to_be_bytes());
        buf.extend_from_slice(&self.v_body_u.to_bits().to_be_bytes());
        buf.extend_from_slice(&self.v_body_v.to_bits().to_be_bytes());
        buf.extend_from_slice(&self.v_body_w.to_bits().to_be_bytes());

        // Accelerations
        buf.extend_from_slice(&self.a_x_pilot.to_bits().to_be_bytes());
        buf.extend_from_slice(&self.a_y_pilot.to_bits().to_be_bytes());
        buf.extend_from_slice(&self.a_z_pilot.to_bits().to_be_bytes());

        // Stall
        buf.extend_from_slice(&self.stall_warning.to_bits().to_be_bytes());
        buf.extend_from_slice(&self.slip_deg.to_bits().to_be_bytes());

        // Engine status
        buf.extend_from_slice(&self.num_engines.to_be_bytes());
        for v in &self.eng_state {
            buf.extend_from_slice(&v.to_be_bytes());
        }
        for v in &self.rpm {
            buf.extend_from_slice(&v.to_bits().to_be_bytes());
        }
        for v in &self.fuel_flow {
            buf.extend_from_slice(&v.to_bits().to_be_bytes());
        }
        for v in &self.fuel_px {
            buf.extend_from_slice(&v.to_bits().to_be_bytes());
        }
        for v in &self.egt {
            buf.extend_from_slice(&v.to_bits().to_be_bytes());
        }
        for v in &self.cht {
            buf.extend_from_slice(&v.to_bits().to_be_bytes());
        }
        for v in &self.mp_osi {
            buf.extend_from_slice(&v.to_bits().to_be_bytes());
        }
        for v in &self.tit {
            buf.extend_from_slice(&v.to_bits().to_be_bytes());
        }
        for v in &self.oil_temp {
            buf.extend_from_slice(&v.to_bits().to_be_bytes());
        }
        for v in &self.oil_px {
            buf.extend_from_slice(&v.to_bits().to_be_bytes());
        }

        // Consumables
        buf.extend_from_slice(&self.num_tanks.to_be_bytes());
        for v in &self.fuel_quantity {
            buf.extend_from_slice(&v.to_bits().to_be_bytes());
        }

        // Gear status
        buf.extend_from_slice(&self.num_wheels.to_be_bytes());
        for v in &self.wow {
            buf.extend_from_slice(&v.to_be_bytes());
        }
        for v in &self.gear_pos {
            buf.extend_from_slice(&v.to_bits().to_be_bytes());
        }
        for v in &self.gear_steer {
            buf.extend_from_slice(&v.to_bits().to_be_bytes());
        }
        for v in &self.gear_compression {
            buf.extend_from_slice(&v.to_bits().to_be_bytes());
        }

        // Environment
        buf.extend_from_slice(&self.cur_time.to_be_bytes());
        buf.extend_from_slice(&self.warp.to_be_bytes());
        buf.extend_from_slice(&self.visibility.to_bits().to_be_bytes());

        // Control surfaces
        buf.extend_from_slice(&self.elevator.to_bits().to_be_bytes());
        buf.extend_from_slice(&self.elevator_trim_tab.to_bits().to_be_bytes());
        buf.extend_from_slice(&self.left_flap.to_bits().to_be_bytes());
        buf.extend_from_slice(&self.right_flap.to_bits().to_be_bytes());
        buf.extend_from_slice(&self.left_aileron.to_bits().to_be_bytes());
        buf.extend_from_slice(&self.right_aileron.to_bits().to_be_bytes());
        buf.extend_from_slice(&self.rudder.to_bits().to_be_bytes());
        buf.extend_from_slice(&self.nose_wheel.to_bits().to_be_bytes());
        buf.extend_from_slice(&self.speedbrake.to_bits().to_be_bytes());
        buf.extend_from_slice(&self.spoilers.to_bits().to_be_bytes());

        debug_assert_eq!(
            buf.len(),
            FG_NET_FDM_SIZE,
            "FgNetFdm::encode() produced wrong size: {} vs expected {}",
            buf.len(),
            FG_NET_FDM_SIZE
        );
        buf
    }
}

// ── FgNetCtrls ────────────────────────────────────────────────────────────────

/// Pilot control inputs packet received **from** FlightGear.
///
/// Matches `FGNetCtrls` in `net_ctrls.hxx` (version 27).
#[derive(Debug, Clone, Default)]
pub struct FgNetCtrls {
    /// Protocol version (must be 27).
    pub version: u32,

    // Aero controls
    /// Aileron input (-1 to 1).
    pub aileron: f64,
    /// Elevator input (-1 to 1).
    pub elevator: f64,
    /// Rudder input (-1 to 1).
    pub rudder: f64,
    /// Aileron trim (-1 to 1).
    pub aileron_trim: f64,
    /// Elevator trim (-1 to 1).
    pub elevator_trim: f64,
    /// Rudder trim (-1 to 1).
    pub rudder_trim: f64,
    /// Flaps position (0 to 1).
    pub flaps: f64,
    /// Spoilers position.
    pub spoilers: f64,
    /// Speedbrake position.
    pub speedbrake: f64,

    // Aero control faults
    /// Flaps power status.
    pub flaps_power: u32,
    /// Flap motor health status.
    pub flap_motor_ok: u32,

    // Engine controls
    /// Number of engines.
    pub num_engines: u32,
    /// Master battery switch per engine.
    pub master_bat: [u32; FG_MAX_ENGINES_CTRLS],
    /// Master alternator switch per engine.
    pub master_alt: [u32; FG_MAX_ENGINES_CTRLS],
    /// Magneto position per engine.
    pub magnetos: [u32; FG_MAX_ENGINES_CTRLS],
    /// Starter power per engine.
    pub starter_power: [u32; FG_MAX_ENGINES_CTRLS],
    /// Throttle position per engine (0 to 1).
    pub throttle: [f64; FG_MAX_ENGINES_CTRLS],
    /// Mixture control per engine (0 to 1).
    pub mixture: [f64; FG_MAX_ENGINES_CTRLS],
    /// Condition lever per engine.
    pub condition: [f64; FG_MAX_ENGINES_CTRLS],
    /// Fuel pump power per engine.
    pub fuel_pump_power: [u32; FG_MAX_ENGINES_CTRLS],
    /// Prop advance per engine.
    pub prop_advance: [f64; FG_MAX_ENGINES_CTRLS],
    /// Feed tank selector per engine.
    pub feed_tank_to: [u32; 4],
    /// Thrust reverser per engine.
    pub reverse: [u32; 4],

    // Engine faults
    /// Engine health status per engine.
    pub engine_ok: [u32; FG_MAX_ENGINES_CTRLS],
    /// Left magneto health per engine.
    pub mag_left_ok: [u32; FG_MAX_ENGINES_CTRLS],
    /// Right magneto health per engine.
    pub mag_right_ok: [u32; FG_MAX_ENGINES_CTRLS],
    /// Spark plug health per engine.
    pub spark_plugs_ok: [u32; FG_MAX_ENGINES_CTRLS],
    /// Oil pressure status per engine.
    pub oil_press_status: [u32; FG_MAX_ENGINES_CTRLS],
    /// Fuel pump health per engine.
    pub fuel_pump_ok: [u32; FG_MAX_ENGINES_CTRLS],

    // Fuel management
    /// Number of fuel tanks.
    pub num_tanks: u32,
    /// Fuel selector valve per tank.
    pub fuel_selector: [u32; FG_MAX_TANKS_CTRLS],
    /// Transfer pump status.
    pub xfer_pump: [u32; 5],
    /// Cross-feed valve.
    pub cross_feed: u32,

    // Brake controls
    /// Left brake input (0 to 1).
    pub brake_left: f64,
    /// Right brake input (0 to 1).
    pub brake_right: f64,
    /// Copilot left brake input (0 to 1).
    pub copilot_brake_left: f64,
    /// Copilot right brake input (0 to 1).
    pub copilot_brake_right: f64,
    /// Parking brake input (0 to 1).
    pub brake_parking: f64,

    // Landing gear
    /// Gear handle position (0=up, 1=down).
    pub gear_handle: u32,

    // Switches
    /// Master avionics switch.
    pub master_avionics: u32,

    // Nav/Comm
    /// Comm radio 1 frequency.
    pub comm_1: f64,
    /// Comm radio 2 frequency.
    pub comm_2: f64,
    /// Nav radio 1 frequency.
    pub nav_1: f64,
    /// Nav radio 2 frequency.
    pub nav_2: f64,

    // Wind/turbulence
    /// Wind speed in knots.
    pub wind_speed_kt: f64,
    /// Wind direction in degrees.
    pub wind_dir_deg: f64,
    /// Turbulence intensity (0 to 1).
    pub turbulence_norm: f64,

    // Temp/pressure
    /// Outside air temperature in °C.
    pub temp_c: f64,
    /// Barometric pressure in inHg.
    pub press_inhg: f64,

    // Environment
    /// Ground elevation in metres.
    pub hground: f64,
    /// Magnetic variation in degrees.
    pub magvar: f64,

    // Hazards
    /// Icing condition flag.
    pub icing: u32,

    // Simulation control
    /// Simulation speed multiplier.
    pub speedup: u32,
    /// Simulation freeze flags.
    pub freeze: u32,

    // Reserved
    /// Reserved fields for future use.
    pub reserved: [u32; RESERVED_SPACE],
}

// ── decode helpers ────────────────────────────────────────────────────────────

fn read_u32(buf: &[u8], offset: usize) -> Result<u32> {
    let bytes = buf
        .get(offset..offset + 4)
        .with_context(|| format!("FgNetCtrls: buffer too short at offset {offset}"))?;
    let arr: [u8; 4] = bytes
        .try_into()
        .with_context(|| format!("FgNetCtrls: u32 conversion failed at offset {offset}"))?;
    Ok(u32::from_be_bytes(arr))
}

fn read_f64(buf: &[u8], offset: usize) -> Result<f64> {
    let bytes = buf
        .get(offset..offset + 8)
        .with_context(|| format!("FgNetCtrls: buffer too short at offset {offset}"))?;
    let arr: [u8; 8] = bytes
        .try_into()
        .with_context(|| format!("FgNetCtrls: f64 conversion failed at offset {offset}"))?;
    let bits = u64::from_be_bytes(arr);
    Ok(f64::from_bits(bits))
}

fn read_u32_array<const N: usize>(buf: &[u8], offset: usize) -> Result<[u32; N]> {
    let mut arr = [0u32; N];
    for (i, item) in arr.iter_mut().enumerate().take(N) {
        *item = read_u32(buf, offset + i * 4)?;
    }
    Ok(arr)
}

fn read_f64_array<const N: usize>(buf: &[u8], offset: usize) -> Result<[f64; N]> {
    let mut arr = [0f64; N];
    for (i, item) in arr.iter_mut().enumerate().take(N) {
        *item = read_f64(buf, offset + i * 8)?;
    }
    Ok(arr)
}

impl FgNetCtrls {
    /// Decode a big-endian byte buffer into an `FgNetCtrls`.
    ///
    /// Returns `Err` unless the packet is exactly [`FG_NET_CTRLS_SIZE`] bytes
    /// and advertises [`FG_NET_CTRLS_VERSION`].
    pub fn decode(buf: &[u8]) -> Result<Self> {
        if buf.len() != FG_NET_CTRLS_SIZE {
            bail!(
                "FgNetCtrls::decode: invalid packet size (got {}, expected {})",
                buf.len(),
                FG_NET_CTRLS_SIZE
            );
        }

        let mut o = 0usize; // running byte offset

        macro_rules! u32 {
            () => {{
                let v = read_u32(buf, o)?;
                o += 4;
                v
            }};
        }
        macro_rules! f64 {
            () => {{
                let v = read_f64(buf, o)?;
                o += 8;
                v
            }};
        }
        macro_rules! u32_arr {
            ($n:literal) => {{
                let v = read_u32_array::<$n>(buf, o)?;
                o += 4 * $n;
                v
            }};
        }
        macro_rules! f64_arr {
            ($n:literal) => {{
                let v = read_f64_array::<$n>(buf, o)?;
                o += 8 * $n;
                v
            }};
        }

        let version = u32!();
        if version != FG_NET_CTRLS_VERSION {
            bail!(
                "FgNetCtrls::decode: unsupported version {} (expected {})",
                version,
                FG_NET_CTRLS_VERSION
            );
        }
        o += 4; // struct padding: align next f64 to 8-byte boundary
        let aileron = f64!();
        let elevator = f64!();
        let rudder = f64!();
        let aileron_trim = f64!();
        let elevator_trim = f64!();
        let rudder_trim = f64!();
        let flaps = f64!();
        let spoilers = f64!();
        let speedbrake = f64!();
        let flaps_power = u32!();
        let flap_motor_ok = u32!();
        let num_engines = u32!();
        o += 4; // struct padding: align u32[4] arrays to 8-byte boundary
        let master_bat = u32_arr!(4);
        let master_alt = u32_arr!(4);
        let magnetos = u32_arr!(4);
        let starter_power = u32_arr!(4);
        let throttle = f64_arr!(4);
        let mixture = f64_arr!(4);
        let condition = f64_arr!(4);
        let fuel_pump_power = u32_arr!(4);
        let prop_advance = f64_arr!(4);
        let feed_tank_to = u32_arr!(4);
        let reverse = u32_arr!(4);
        let engine_ok = u32_arr!(4);
        let mag_left_ok = u32_arr!(4);
        let mag_right_ok = u32_arr!(4);
        let spark_plugs_ok = u32_arr!(4);
        let oil_press_status = u32_arr!(4);
        let fuel_pump_ok = u32_arr!(4);
        let num_tanks = u32!();
        let fuel_selector = u32_arr!(8);
        let xfer_pump = u32_arr!(5);
        let cross_feed = u32!();
        o += 4; // struct padding: align next f64 to 8-byte boundary
        let brake_left = f64!();
        let brake_right = f64!();
        let copilot_brake_left = f64!();
        let copilot_brake_right = f64!();
        let brake_parking = f64!();
        let gear_handle = u32!();
        let master_avionics = u32!();
        // gear_handle(4) + master_avionics(4) = 8 bytes, already 8-byte aligned
        let comm_1 = f64!();
        let comm_2 = f64!();
        let nav_1 = f64!();
        let nav_2 = f64!();
        let wind_speed_kt = f64!();
        let wind_dir_deg = f64!();
        let turbulence_norm = f64!();
        let temp_c = f64!();
        let press_inhg = f64!();
        let hground = f64!();
        let magvar = f64!();
        let icing = u32!();
        let speedup = u32!();
        let freeze = u32!();
        let reserved = u32_arr!(25);

        debug_assert_eq!(
            o, FG_NET_CTRLS_SIZE,
            "FgNetCtrls::decode consumed {} bytes, expected {}",
            o, FG_NET_CTRLS_SIZE
        );

        Ok(Self {
            version,
            aileron,
            elevator,
            rudder,
            aileron_trim,
            elevator_trim,
            rudder_trim,
            flaps,
            spoilers,
            speedbrake,
            flaps_power,
            flap_motor_ok,
            num_engines,
            master_bat,
            master_alt,
            magnetos,
            starter_power,
            throttle,
            mixture,
            condition,
            fuel_pump_power,
            prop_advance,
            feed_tank_to,
            reverse,
            engine_ok,
            mag_left_ok,
            mag_right_ok,
            spark_plugs_ok,
            oil_press_status,
            fuel_pump_ok,
            num_tanks,
            fuel_selector,
            xfer_pump,
            cross_feed,
            brake_left,
            brake_right,
            copilot_brake_left,
            copilot_brake_right,
            brake_parking,
            gear_handle,
            master_avionics,
            comm_1,
            comm_2,
            nav_1,
            nav_2,
            wind_speed_kt,
            wind_dir_deg,
            turbulence_norm,
            temp_c,
            press_inhg,
            hground,
            magvar,
            icing,
            speedup,
            freeze,
            reserved,
        })
    }

    /// Encode `FgNetCtrls` to a big-endian byte buffer (for testing roundtrips).
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(FG_NET_CTRLS_SIZE);

        macro_rules! pu32 {
            ($v:expr) => {
                buf.extend_from_slice(&($v).to_be_bytes());
            };
        }
        macro_rules! pf64 {
            ($v:expr) => {
                buf.extend_from_slice(&($v).to_bits().to_be_bytes());
            };
        }

        pu32!(self.version);
        // C layout contains 4 bytes of padding after version to align f64 fields.
        pu32!(0u32);
        pf64!(self.aileron);
        pf64!(self.elevator);
        pf64!(self.rudder);
        pf64!(self.aileron_trim);
        pf64!(self.elevator_trim);
        pf64!(self.rudder_trim);
        pf64!(self.flaps);
        pf64!(self.spoilers);
        pf64!(self.speedbrake);
        pu32!(self.flaps_power);
        pu32!(self.flap_motor_ok);
        pu32!(self.num_engines);
        // C layout contains 4 bytes of padding before u32 arrays.
        pu32!(0u32);
        for v in &self.master_bat {
            pu32!(v);
        }
        for v in &self.master_alt {
            pu32!(v);
        }
        for v in &self.magnetos {
            pu32!(v);
        }
        for v in &self.starter_power {
            pu32!(v);
        }
        for v in &self.throttle {
            pf64!(v);
        }
        for v in &self.mixture {
            pf64!(v);
        }
        for v in &self.condition {
            pf64!(v);
        }
        for v in &self.fuel_pump_power {
            pu32!(v);
        }
        for v in &self.prop_advance {
            pf64!(v);
        }
        for v in &self.feed_tank_to {
            pu32!(v);
        }
        for v in &self.reverse {
            pu32!(v);
        }
        for v in &self.engine_ok {
            pu32!(v);
        }
        for v in &self.mag_left_ok {
            pu32!(v);
        }
        for v in &self.mag_right_ok {
            pu32!(v);
        }
        for v in &self.spark_plugs_ok {
            pu32!(v);
        }
        for v in &self.oil_press_status {
            pu32!(v);
        }
        for v in &self.fuel_pump_ok {
            pu32!(v);
        }
        pu32!(self.num_tanks);
        for v in &self.fuel_selector {
            pu32!(v);
        }
        for v in &self.xfer_pump {
            pu32!(v);
        }
        pu32!(self.cross_feed);
        // C layout contains 4 bytes of padding before brake f64 fields.
        pu32!(0u32);
        pf64!(self.brake_left);
        pf64!(self.brake_right);
        pf64!(self.copilot_brake_left);
        pf64!(self.copilot_brake_right);
        pf64!(self.brake_parking);
        pu32!(self.gear_handle);
        pu32!(self.master_avionics);
        pf64!(self.comm_1);
        pf64!(self.comm_2);
        pf64!(self.nav_1);
        pf64!(self.nav_2);
        pf64!(self.wind_speed_kt);
        pf64!(self.wind_dir_deg);
        pf64!(self.turbulence_norm);
        pf64!(self.temp_c);
        pf64!(self.press_inhg);
        pf64!(self.hground);
        pf64!(self.magvar);
        pu32!(self.icing);
        pu32!(self.speedup);
        pu32!(self.freeze);
        for v in &self.reserved {
            pu32!(v);
        }

        debug_assert_eq!(
            buf.len(),
            FG_NET_CTRLS_SIZE,
            "FgNetCtrls::encode() produced wrong size: {} vs expected {}",
            buf.len(),
            FG_NET_CTRLS_SIZE
        );
        buf
    }
}

// ── FlightGearBridge ──────────────────────────────────────────────────────────

use crate::config::FlightGearConfig;

/// UDP bridge metadata and controls ingress for FlightGear integration.
///
/// - Tracks the configured FlightGear FDM destination address.
/// - Receives `FGNetCtrls` packets via non-blocking UDP.
pub struct FlightGearBridge {
    ctrls_socket: UdpSocket,
    fdm_target: std::net::SocketAddr,
}

impl FlightGearBridge {
    /// Construct the bridge: bind the controls receive socket and parse the FDM destination.
    pub fn new(config: &FlightGearConfig) -> Result<Self> {
        let ctrls_bind = format!("{}:{}", config.ctrls_recv_addr, config.ctrls_recv_port);

        let addr: std::net::SocketAddr = ctrls_bind
            .parse()
            .with_context(|| format!("FlightGear: invalid bind address: {ctrls_bind}"))?;
        let domain = if addr.is_ipv4() {
            socket2::Domain::IPV4
        } else {
            socket2::Domain::IPV6
        };
        let socket =
            socket2::Socket::new(domain, socket2::Type::DGRAM, Some(socket2::Protocol::UDP))
                .with_context(|| "FlightGear: failed to create ctrls socket")?;

        socket
            .set_reuse_address(true)
            .context("FlightGear: failed to set SO_REUSEADDR")?;
        socket
            .bind(&addr.into())
            .with_context(|| format!("FlightGear: failed to bind ctrls socket on {ctrls_bind}"))?;

        let ctrls_socket: UdpSocket = socket.into();

        ctrls_socket
            .set_nonblocking(true)
            .context("FlightGear: failed to set ctrls socket non-blocking")?;

        let fdm_target: std::net::SocketAddr =
            format!("{}:{}", config.fdm_send_addr, config.fdm_send_port)
                .parse()
                .with_context(|| {
                    format!(
                        "FlightGear: invalid fdm target address {}:{}",
                        config.fdm_send_addr, config.fdm_send_port
                    )
                })?;

        debug!(
            target: "flightgear.bridge_bound",
            fdm_port = config.fdm_send_port,
            ctrls_port = config.ctrls_recv_port,
            "FlightGear bridge bound"
        );

        Ok(Self {
            ctrls_socket,
            fdm_target,
        })
    }

    /// Return the FDM destination address (for the interpolation thread).
    pub fn fdm_dest_addr(&self) -> Option<std::net::SocketAddr> {
        Some(self.fdm_target)
    }

    /// Returns the local address the ctrls socket is bound to.
    ///
    /// Used by tests to discover the ephemeral port for loopback packet injection.
    pub fn ctrls_local_addr(&self) -> std::io::Result<std::net::SocketAddr> {
        self.ctrls_socket.local_addr()
    }

    /// Attempt a non-blocking receive of one `FgNetCtrls` packet.
    ///
    /// Returns `Ok(None)` if no packet is available (EAGAIN/EWOULDBLOCK).
    /// Returns `Err` on real socket errors.
    pub fn recv_ctrls_nonblocking(&self) -> Result<Option<FgNetCtrls>> {
        let mut buf = [0u8; FG_NET_CTRLS_SIZE + 64];
        match self.ctrls_socket.recv_from(&mut buf) {
            Ok((n, src)) => {
                debug!(
                    target: "flightgear.recv_ctrls",
                    bytes = n,
                    source = %src,
                    "Received FGNetCtrls packet"
                );
                match FgNetCtrls::decode(&buf[..n]) {
                    Ok(ctrls) => Ok(Some(ctrls)),
                    Err(e) => {
                        warn!(
                            target: "flightgear.recv_ctrls_invalid",
                            bytes = n,
                            source = %src,
                            error = %e,
                            "Dropping invalid FGNetCtrls packet"
                        );
                        Ok(None)
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
            Err(e) => {
                error!(
                    target: "flightgear.recv_failed",
                    error = %e,
                    "Failed to receive FGNetCtrls packet"
                );
                Err(anyhow::anyhow!("FlightGear: recv_ctrls failed: {e}"))
            }
        }
    }
}

// ── Read helpers for FDM (used by contract tests) ─────────────────────────────

/// Test helper for reading a big-endian `f64` at a fixed offset.
///
/// # Panics
///
/// Panics if `buf[offset..offset + 8]` is out of bounds.
pub fn read_f64_at(buf: &[u8], offset: usize) -> f64 {
    let arr: [u8; 8] = buf[offset..offset + 8]
        .try_into()
        .expect("buffer slice length must match f64 size");
    let bits = u64::from_be_bytes(arr);
    f64::from_bits(bits)
}

/// Test helper for reading a big-endian `f32` at a fixed offset.
///
/// # Panics
///
/// Panics if `buf[offset..offset + 4]` is out of bounds.
pub fn read_f32_at(buf: &[u8], offset: usize) -> f32 {
    let arr: [u8; 4] = buf[offset..offset + 4]
        .try_into()
        .expect("buffer slice length must match f32 size");
    let bits = u32::from_be_bytes(arr);
    f32::from_bits(bits)
}

/// Test helper for reading a big-endian `u32` at a fixed offset.
///
/// # Panics
///
/// Panics if `buf[offset..offset + 4]` is out of bounds.
pub fn read_u32_at(buf: &[u8], offset: usize) -> u32 {
    let arr: [u8; 4] = buf[offset..offset + 4]
        .try_into()
        .expect("buffer slice length must match u32 size");
    u32::from_be_bytes(arr)
}

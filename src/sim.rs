//! Tick-driven simulation runtime.
//!
//! Steps entities, updates guidance state, and publishes DIS output.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use metrics::{counter, gauge, histogram};
use rayon::prelude::*;
use tracing::{debug, error, info, warn};

use crate::config::Waypoint;
use crate::dis::DisPublisher;
use crate::entity::{EntityState, EntityStatus};
use crate::fdm::FdmHandle;
use crate::flightgear::FlightGearBridge;
use crate::owp::OwpPublisherHandle;

// ─── Geometry helpers ────────────────────────────────────────────────────────

const EARTH_RADIUS_M: f64 = 6_371_000.0;
const M_TO_FT: f64 = 1.0 / 0.3048;
const FG_LAT_COS_EPSILON: f64 = 1.0e-6;
const SIM_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);

/// Duration over which the FG interpolation thread blends from the old visual
/// position to the new DR path after a sim tick publishes fresh FDM state.
/// Tuned for a 2 Hz sim tick (500 ms): 300 ms covers 60% of the tick interval,
/// leaving 200 ms of pure DR before the next update.
const FG_CONVERGE_SECONDS: f64 = 0.30;

/// Snapshot of the visual position at the moment a new FDM state arrives,
/// used for convergence smoothing to eliminate snap-back glitches.
#[derive(Clone, Debug)]
struct FgConvergeSnapshot {
    latitude_deg: f64,
    longitude_deg: f64,
    altitude_m: f64,
    altitude_msl_m: f64,
    roll_deg: f64,
    pitch_deg: f64,
    yaw_deg: f64,
}

/// Shared state published by the sim tick and consumed by the fg-interp thread.
#[derive(Clone, Debug)]
struct FgInterpState {
    /// Latest FDM truth from JSBSim.
    state: EntityState,
    /// Wall-clock time when this state was published.
    timestamp: Instant,
    /// Visual position captured just before the state update, for convergence
    /// smoothing.  `None` on the very first publish (no prior visual position).
    converge: Option<FgConvergeSnapshot>,
}

fn haversine_distance_m(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let dlat = (lat2 - lat1).to_radians();
    let dlon = (lon2 - lon1).to_radians();
    let a = (dlat / 2.0).sin().powi(2)
        + lat1.to_radians().cos() * lat2.to_radians().cos() * (dlon / 2.0).sin().powi(2);
    EARTH_RADIUS_M * 2.0 * a.clamp(0.0, 1.0).sqrt().asin()
}

fn bearing_deg(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let lat1 = lat1.to_radians();
    let lat2 = lat2.to_radians();
    let dlon = (lon2 - lon1).to_radians();
    let y = dlon.sin() * lat2.cos();
    let x = lat1.cos() * lat2.sin() - lat1.sin() * lat2.cos() * dlon.cos();
    (y.atan2(x).to_degrees() + 360.0) % 360.0
}

fn clone_fg_interp_state(
    fg_state: &Arc<Mutex<Option<FgInterpState>>>,
) -> std::result::Result<Option<FgInterpState>, ()> {
    match fg_state.lock() {
        Ok(lock) => Ok(lock.clone()),
        Err(_) => Err(()),
    }
}

/// Hermite smoothstep: `t² × (3 − 2t)`.  Maps `[0, 1] → [0, 1]` with zero
/// first-derivative at both endpoints, giving a smooth ease-in / ease-out.
fn smoothstep(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn extrapolate_fg_state(state: &EntityState, dt: f64) -> EntityState {
    let mut interp = state.clone();

    // Extrapolate orientation using body angular rates.
    interp.roll_deg = state.roll_deg + state.roll_rate_rps.to_degrees() * dt;
    interp.pitch_deg = state.pitch_deg + state.pitch_rate_rps.to_degrees() * dt;
    let hdg_delta = state.yaw_rate_rps.to_degrees() * dt;
    interp.yaw_deg = (state.yaw_deg + hdg_delta + 360.0) % 360.0;

    // Extrapolate position with velocity rotated by half heading change.
    let half_hdg = (hdg_delta * 0.5).to_radians();
    let cos_h = half_hdg.cos();
    let sin_h = half_hdg.sin();
    let vn = state.velocity_north_mps;
    let ve = state.velocity_east_mps;
    let dlat = (vn * cos_h - ve * sin_h) * dt / 111_320.0;

    let lat_cos = state.latitude_deg.to_radians().cos();
    let lat_cos_safe = if lat_cos.abs() < FG_LAT_COS_EPSILON {
        FG_LAT_COS_EPSILON
    } else {
        lat_cos
    };

    let dlon = (vn * sin_h + ve * cos_h) * dt / (111_320.0 * lat_cos_safe);
    let dalt = -state.velocity_down_mps * dt;
    interp.latitude_deg += dlat;
    interp.longitude_deg += dlon;
    interp.altitude_m += dalt;
    interp.altitude_msl_m += dalt;

    interp
}

// ─── Runtime entity model ────────────────────────────────────────────────────

/// A live simulation entity.
pub enum RuntimeEntity {
    /// Flying platform with JSBSim FDM and waypoint navigation.
    Flying {
        /// FDM connection handle for stepping and state reads.
        handle: Box<dyn FdmHandle + Send>,
        /// Current kinematic state.
        state: EntityState,
        /// Lifecycle status (active or dead).
        status: EntityStatus,
        /// Waypoints for autopilot navigation.
        waypoints: Vec<Waypoint>,
        /// Index of the active waypoint in the flight plan.
        active_wp: usize,
        /// Optional FlightGear bridge — sends FDM state for cockpit rendering.
        bridge: Option<FlightGearBridge>,
        /// Previous ECEF velocity for acceleration computation.
        prev_ecef_vel: Option<(f32, f32, f32)>,
        /// Last heading setpoint commanded to the autopilot (degrees true).
        /// Used to suppress redundant heading updates that cause AP oscillation.
        last_hdg_setpoint: Option<f64>,

        /// Manual override aggression factor (1–10).
        override_aggression: f64,
        /// Throttle threshold for manual override transitions.
        ///
        /// Manual mode engages when throttle is greater than this threshold and
        /// disengages when throttle is less than this threshold.
        autopilot_threshold: f64,
        /// Max age in seconds of the most recent valid FlightGear controls
        /// packet while manual override remains active.
        override_timeout_secs: f64,
        /// Time when the most recent valid FlightGear controls packet was
        /// received.
        last_fg_ctrls_at: Option<Instant>,
    },
    /// Fixed ground site — static position, no FDM.
    Fixed {
        /// Current kinematic state.
        state: EntityState,
        /// Lifecycle status (active or dead).
        status: EntityStatus,
    },
}

impl RuntimeEntity {
    /// Return a reference to the entity's kinematic state.
    pub fn state(&self) -> &EntityState {
        match self {
            RuntimeEntity::Flying { state, .. } | RuntimeEntity::Fixed { state, .. } => state,
        }
    }

    /// Return the entity's lifecycle status.
    pub fn status(&self) -> EntityStatus {
        match self {
            RuntimeEntity::Flying { status, .. } | RuntimeEntity::Fixed { status, .. } => *status,
        }
    }

    /// Return `true` if this is a flying (JSBSim-backed) entity.
    pub fn is_flying(&self) -> bool {
        matches!(self, RuntimeEntity::Flying { .. })
    }

    /// Return `true` if this is a fixed (static position) entity.
    pub fn is_fixed(&self) -> bool {
        matches!(self, RuntimeEntity::Fixed { .. })
    }
}

// ─── Simulation ──────────────────────────────────────────────────────────────

/// Runtime simulation engine coordinating FDM stepping and DIS publication.
pub struct Simulation {
    entities: Vec<RuntimeEntity>,
    dis: DisPublisher,
    owp_publisher: Option<OwpPublisherHandle>,
    waypoint_threshold_m: f64,
    blue_entity_id: u16,
}

impl Simulation {
    /// Construct a simulation with runtime entities, DIS publisher, and waypoint arrival radius.
    pub fn new(
        entities: Vec<RuntimeEntity>,
        dis: DisPublisher,
        owp_publisher: Option<OwpPublisherHandle>,
        waypoint_threshold_m: f64,
        blue_entity_id: u16,
    ) -> Self {
        Self {
            entities,
            dis,
            owp_publisher,
            waypoint_threshold_m,
            blue_entity_id,
        }
    }

    /// No-op — JSBSim is driven by iterate(), not free-running.
    pub fn start_fdms(&mut self) -> Result<()> {
        for entity in &mut self.entities {
            if let RuntimeEntity::Flying {
                handle,
                state,
                status,
                ..
            } = entity
            {
                if *status != EntityStatus::Active {
                    continue;
                }
                if let Err(e) = handle.start() {
                    error!(entity_id = state.entity_id, error = %e, "fdm.start failed");
                    *status = EntityStatus::Dead;
                }
            }
        }
        Ok(())
    }

    /// Run the simulation loop at `tick_hz` until `running` is cleared.
    ///
    /// For the first `settle_secs`, control writes are suppressed while stepping,
    /// state reads, and DIS publication continue.
    pub fn run(
        &mut self,
        running: &Arc<AtomicBool>,
        tick_hz: f64,
        settle_secs: f64,
        last_tick_epoch_secs: &Arc<std::sync::atomic::AtomicU64>,
    ) -> Result<()> {
        let tick_duration = Duration::from_secs_f64(1.0 / tick_hz);
        let dt_sec = 1.0 / tick_hz;
        let settle_duration = Duration::from_secs_f64(settle_secs.max(0.0));
        let settle_until = Instant::now() + settle_duration;

        let flying_count = self.entities.iter().filter(|e| e.is_flying()).count();
        let fixed_count = self.entities.iter().filter(|e| e.is_fixed()).count();

        info!(
            flying = flying_count,
            fixed = fixed_count,
            total = self.entities.len(),
            tick_hz,
            settle_secs,
            waypoint_threshold_m = self.waypoint_threshold_m,
            "sim.run started"
        );

        // Initialize metrics so they appear in Prometheus even before any events occur
        gauge!("supercell_entities_active").set(self.entities.len() as f64);
        counter!("supercell_waypoints_reached_total").increment(0);
        counter!("supercell_fdm_errors_total", "operation" => "step").increment(0);
        counter!("supercell_fdm_errors_total", "operation" => "read").increment(0);
        counter!("supercell_dis_publish_errors_total").increment(0);

        // FlightGear FDM interpolation thread — sends extrapolated FDM at 60 Hz
        // between sim ticks for smooth cockpit rendering.
        let fg_state: Arc<Mutex<Option<FgInterpState>>> = Arc::new(Mutex::new(None));
        let fg_state_writer = fg_state.clone();

        // The interpolation thread consumes a single shared state snapshot, so this
        // runtime path assumes at most one FlightGear-bridged flying entity.
        let fg_bridge_count = self
            .entities
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    RuntimeEntity::Flying {
                        bridge: Some(_),
                        ..
                    }
                )
            })
            .count();
        debug_assert!(
            fg_bridge_count <= 1,
            "fg interpolation state assumes <= 1 bridged entity, found {fg_bridge_count}"
        );

        let fg_send_addr = self.entities.iter().find_map(|e| {
            if let RuntimeEntity::Flying {
                bridge: Some(fg), ..
            } = e
            {
                fg.fdm_dest_addr()
            } else {
                None
            }
        });

        if let Some(dest_addr) = fg_send_addr {
            let fg_state_reader = fg_state.clone();
            let fg_running = running.clone();
            let fg_interp_spawn_result = std::thread::Builder::new()
                .name("fg-interp".into())
                .spawn(move || {
                    let sock = match || -> anyhow::Result<std::net::UdpSocket> {
                        let addr: std::net::SocketAddr = "0.0.0.0:0".parse().unwrap();
                        let socket = socket2::Socket::new(
                            socket2::Domain::IPV4,
                            socket2::Type::DGRAM,
                            Some(socket2::Protocol::UDP),
                        )
                        .context("create fg-interp socket")?;
                        socket.set_reuse_address(true).context("set SO_REUSEADDR")?;
                        socket.bind(&addr.into()).context("bind fg-interp socket")?;
                        Ok(socket.into())
                    }() {
                        Ok(s) => s,
                        Err(e) => {
                            warn!(error = %e, "fg-interp bind failed; interpolation disabled");
                            return;
                        }
                    };
                    let interval = Duration::from_millis(16); // ~60 Hz
                    debug!("fg-interp thread started at 60 Hz, dest={}", dest_addr);

                    while fg_running.load(Ordering::SeqCst) {
                        std::thread::sleep(interval);
                        let Ok(snapshot) = clone_fg_interp_state(&fg_state_reader) else {
                            warn!("fg-interp shared state poisoned; stopping interpolation thread");
                            break;
                        };

                        if let Some(interp_state) = snapshot {
                            let elapsed = interp_state.timestamp.elapsed().as_secs_f64();
                            let mut interp = extrapolate_fg_state(&interp_state.state, elapsed);

                            // Convergence smoothing: blend from the old visual
                            // position to the new DR path over FG_CONVERGE_SECONDS
                            // using a Hermite smoothstep, eliminating visible
                            // snap-back when the sim tick publishes a new state.
                            if let Some(ref conv) = interp_state.converge
                                && elapsed < FG_CONVERGE_SECONDS
                            {
                                let s = smoothstep(elapsed / FG_CONVERGE_SECONDS);
                                interp.latitude_deg = conv.latitude_deg
                                    + (interp.latitude_deg - conv.latitude_deg) * s;
                                interp.longitude_deg = conv.longitude_deg
                                    + (interp.longitude_deg - conv.longitude_deg) * s;
                                interp.altitude_m =
                                    conv.altitude_m + (interp.altitude_m - conv.altitude_m) * s;
                                interp.altitude_msl_m = conv.altitude_msl_m
                                    + (interp.altitude_msl_m - conv.altitude_msl_m) * s;

                                // Blend orientation with wraparound awareness.
                                // Roll (can wrap through ±180°)
                                let mut roll_diff = interp.roll_deg - conv.roll_deg;
                                if roll_diff > 180.0 {
                                    roll_diff -= 360.0;
                                }
                                if roll_diff < -180.0 {
                                    roll_diff += 360.0;
                                }
                                interp.roll_deg = conv.roll_deg + roll_diff * s;

                                // Pitch (small range, no wrap needed)
                                interp.pitch_deg =
                                    conv.pitch_deg + (interp.pitch_deg - conv.pitch_deg) * s;

                                // Heading (wraps at 0°/360°)
                                let mut hdg_diff = interp.yaw_deg - conv.yaw_deg;
                                if hdg_diff > 180.0 {
                                    hdg_diff -= 360.0;
                                }
                                if hdg_diff < -180.0 {
                                    hdg_diff += 360.0;
                                }
                                interp.yaw_deg = (conv.yaw_deg + hdg_diff * s + 360.0) % 360.0;
                            }

                            let fdm = crate::flightgear::FgNetFdm::from_entity_state(&interp);
                            let _ = sock.send_to(&fdm.encode(), dest_addr);
                        }
                    }
                    debug!("fg-interp thread stopped");
                });
            if let Err(e) = fg_interp_spawn_result {
                warn!(error = %e, "failed to spawn fg-interp thread; interpolation disabled");
            }
        }

        let mut tick: u64 = 0;
        let mut last_heartbeat_write: Option<Instant> = None;

        while running.load(Ordering::SeqCst) {
            let t_start = Instant::now();
            tick += 1;
            let now = Instant::now();
            let in_settle_phase = now < settle_until;
            let waypoint_threshold_m = self.waypoint_threshold_m;

            if last_heartbeat_write
                .is_none_or(|last| now.duration_since(last) >= SIM_HEARTBEAT_INTERVAL)
            {
                let epoch_secs = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                last_tick_epoch_secs.store(epoch_secs, Ordering::Relaxed);
                last_heartbeat_write = Some(now);
            }

            // --- Per-entity: set AP targets, step, read state ---
            self.entities.par_iter_mut().for_each(|entity| {
                if let RuntimeEntity::Flying { handle, state, status, waypoints, active_wp, bridge, prev_ecef_vel, last_hdg_setpoint, override_aggression, autopilot_threshold, override_timeout_secs, last_fg_ctrls_at } = entity {
                    if *status != EntityStatus::Active { return; }
                    let entity_id = state.entity_id;

                    // ── Receive FlightGear controls (before AP so we have fresh inputs) ──
                    if let Some(fg) = bridge.as_ref() {
                        let mut latest_ctrls = None;
                        loop {
                            match fg.recv_ctrls_nonblocking() {
                                Ok(Some(ctrls)) => { latest_ctrls = Some(ctrls); }
                                Ok(None) => break,
                                Err(e) => {
                                    error!(entity_id, tick, error = %e, "flightgear recv_ctrls failed");
                                    break;
                                }
                            }
                        }
                        if let Some(ctrls) = latest_ctrls {
                            state.fg_aileron = ctrls.aileron;
                            state.fg_elevator = ctrls.elevator;
                            state.fg_rudder = ctrls.rudder;
                            state.fg_elevator_trim = ctrls.elevator_trim;
                            state.fg_throttle = ctrls.throttle[0];
                            *last_fg_ctrls_at = Some(Instant::now());

                            // Throttle-based override:
                            // throttle > threshold → manual mode (AP-assisted heading/altitude).
                            // throttle < threshold → full autopilot waypoint navigation.
                            let engage_manual = ctrls.throttle[0] > *autopilot_threshold;
                            let disengage_manual = ctrls.throttle[0] < *autopilot_threshold;

                            if engage_manual && !state.manual_override {
                                state.manual_override = true;
                                info!(entity_id, tick,
                                    throttle = format!("{:.3}", ctrls.throttle[0]),
                                    autopilot_threshold = format!("{:.3}", *autopilot_threshold),
                                    "autopilot.MANUAL — throttle engaged");
                            } else if disengage_manual && state.manual_override {
                                state.manual_override = false;
                                state.manual_alt_offset_m = 0.0;
                                *last_hdg_setpoint = None;
                                info!(entity_id, tick,
                                    throttle = format!("{:.3}", ctrls.throttle[0]),
                                    autopilot_threshold = format!("{:.3}", *autopilot_threshold),
                                    "autopilot.ENGAGED — throttle below threshold");
                            }

                            if tick.is_multiple_of(10) {
                                debug!(
                                    entity_id, tick,
                                    manual = state.manual_override,
                                    ail = format!("{:.3}", ctrls.aileron),
                                    ele = format!("{:.3}", ctrls.elevator),
                                    rud = format!("{:.3}", ctrls.rudder),
                                    thr = format!("{:.3}", ctrls.throttle[0]),
                                    trim = format!("{:.3}", ctrls.elevator_trim),
                                    alt_off = format!("{:.1}", state.manual_alt_offset_m),
                                    "flightgear.ctrls"
                                );
                            }
                        } else if state.manual_override {
                            let controls_stale = last_fg_ctrls_at
                                .map(|t| t.elapsed().as_secs_f64() > *override_timeout_secs)
                                .unwrap_or(true);
                            if controls_stale {
                                state.manual_override = false;
                                state.manual_alt_offset_m = 0.0;
                                *last_hdg_setpoint = None;
                                info!(
                                    entity_id,
                                    tick,
                                    override_timeout_secs = format!("{:.3}", *override_timeout_secs),
                                    "autopilot.ENGAGED — stale FlightGear controls"
                                );
                            }
                        }
                    } else if state.manual_override {
                        // No FG bridge at all — force autopilot on
                        state.manual_override = false;
                        state.manual_alt_offset_m = 0.0;
                        *last_hdg_setpoint = None;
                    }

                    // ── Set flight controls ──
                    let mut set_control_property = |property: &str, value: f64| -> bool {
                        if let Err(e) = handle.set_property(property, value) {
                            error!(
                                entity_id,
                                tick,
                                property,
                                value,
                                error = %e,
                                "entity.dead — JSBSim control write failure"
                            );
                            *status = EntityStatus::Dead;
                            return false;
                        }
                        true
                    };

                    if !in_settle_phase {
                        if state.manual_override {
                            // ── Manual mode ──
                            // Mirrors the heading pattern: stick leads actual by up to
                            // a clamped amount, centered stick snaps to current value.
                            let agg = *override_aggression;

                            // ── Heading: stick leads actual by up to 30° ──
                            let new_hdg = if state.fg_aileron.abs() > 0.05 {
                                let desired = last_hdg_setpoint.unwrap_or(state.yaw_deg)
                                    + state.fg_aileron * agg * 1.0;
                                let desired = (desired + 360.0) % 360.0;
                                let mut diff = desired - state.yaw_deg;
                                if diff > 180.0 { diff -= 360.0; }
                                if diff < -180.0 { diff += 360.0; }
                                let max_lead = 20.0;
                                let clamped_diff = diff.clamp(-max_lead, max_lead);
                                (state.yaw_deg + clamped_diff + 360.0) % 360.0
                            } else {
                                // Stick centered: hold current heading
                                state.yaw_deg
                            };
                            if !set_control_property("ap/heading_setpoint", new_hdg) {
                                return;
                            }
                            if !set_control_property("ap/heading_hold", 1.0) {
                                return;
                            }
                            if !set_control_property("ap/attitude_hold", 1.0) {
                                return;
                            }
                            *last_hdg_setpoint = Some(new_hdg);

                            // ── Altitude: stick leads actual by up to ±150m (≈500 ft) ──
                            //
                            // Elevator active: adjust offset from actual altitude, clamped.
                            //   Pull back (+) = climb, push forward (−) = descend.
                            //   At agg=8, full stick: 1.0 × 8 × 3.0 = 24 m/s rate.
                            // Elevator centered: snap to actual altitude (hold altitude).
                            // Trim: adds persistent bias so the "hold" altitude drifts.
                            //   At agg=8, full trim past deadband: ~0.5 × 8 × 1.5 = 6 m/s.
                            let max_alt_lead_m = 150.0; // ≈ 500 ft

                            // Trim: persistent climb/descend bias
                            let trim = state.fg_elevator_trim;
                            let trim_deadband = 0.10;
                            let trim_neutral = 0.4;
                            let trim_input = trim - trim_neutral;
                            if trim_input.abs() > trim_deadband {
                                let trim_active = if trim_input > 0.0 { trim_input - trim_deadband } else { trim_input + trim_deadband };
                                state.manual_alt_offset_m += trim_active * agg * 1.5 * dt_sec;
                            }

                            if state.fg_elevator.abs() > 0.05 {
                                // Stick active: adjust offset, clamp to ±max_lead from actual.
                                // FG elevator: -1.0 = full aft (pull back), +1.0 = push forward.
                                // Negate so pull-back = climb (+offset).
                                state.manual_alt_offset_m += -state.fg_elevator * agg * 3.0 * dt_sec;
                                state.manual_alt_offset_m = state.manual_alt_offset_m.clamp(-max_alt_lead_m, max_alt_lead_m);
                            } else {
                                // Stick centered: decay offset toward zero (hold current altitude)
                                // Snap to zero instantly, matching heading behavior.
                                state.manual_alt_offset_m = 0.0;
                            }

                            let target_alt_msl_m = state.altitude_msl_m + state.manual_alt_offset_m;
                            let target_agl_ft = (target_alt_msl_m - state.terrain_elevation_m) * M_TO_FT;
                            if !set_control_property("ap/altitude_hold", 1.0) {
                                return;
                            }
                            if !set_control_property("ap/altitude_setpoint", target_agl_ft) {
                                return;
                            }

                            // Throttle: direct engine power
                            if !set_control_property("fcs/throttle-cmd-norm", state.fg_throttle) {
                                return;
                            }
                        } else if !waypoints.is_empty() {
                            // ── Autopilot mode: waypoint navigation ──
                            let wp = &waypoints[*active_wp];

                            // Altitude hold from waypoint
                            let target_agl_ft = (wp.altitude_m - state.terrain_elevation_m) * M_TO_FT;
                            if !set_control_property("ap/altitude_hold", 1.0) {
                                return;
                            }
                            if !set_control_property("ap/altitude_setpoint", target_agl_ft) {
                                return;
                            }

                            // Heading hold toward waypoint
                            let desired_hdg = bearing_deg(
                                state.latitude_deg, state.longitude_deg,
                                wp.latitude_deg, wp.longitude_deg,
                            );
                            let need_update = match *last_hdg_setpoint {
                                Some(prev) => {
                                    let mut diff: f64 = (desired_hdg - prev).abs();
                                    if diff > 180.0 { diff = 360.0 - diff; }
                                    diff > 2.0
                                }
                                None => true,
                            };
                            if need_update {
                                if !set_control_property("ap/heading_setpoint", desired_hdg) {
                                    return;
                                }
                                *last_hdg_setpoint = Some(desired_hdg);
                            }
                            if !set_control_property("ap/heading_hold", 1.0) {
                                return;
                            }
                            if !set_control_property("ap/attitude_hold", 1.0) {
                                return;
                            }
                        }
                    } else if tick.is_multiple_of(20) {
                        debug!(entity_id, tick, settle_secs, "sim.settle_phase active — skipping control writes");
                    }

                    // ── Step JSBSim ──
                    if let Err(e) = handle.step(dt_sec) {
                        *status = EntityStatus::Dead;
                        counter!("supercell_fdm_errors_total", "operation" => "step").increment(1);
                        info!(entity_id, tick, error = %e, "entity.dead — FDM step failure");
                        return;
                    }

                    // ── Read state ──
                    match handle.read_state() {
                        Ok(mut new_state) => {
                            if tick % 20 == 1 {
                                let gs = (new_state.velocity_north_mps.powi(2)
                                    + new_state.velocity_east_mps.powi(2)).sqrt();
                                debug!(
                                    entity_id, tick,
                                    alt_m = format!("{:.1}", new_state.altitude_m),
                                    hdg = format!("{:.1}", new_state.yaw_deg),
                                    kt = format!("{:.0}", gs * 1.944),
                                    vcas = format!("{:.1}", new_state.vcas_kts),
                                    lat = format!("{:.4}", new_state.latitude_deg),
                                    lon = format!("{:.4}", new_state.longitude_deg),
                                    wp = *active_wp,
                                    "flight.state"
                                );
                            }

                            // Compute ECEF velocity for this tick
                            let (vx, vy, vz) = crate::dis::ned_to_ecef_velocity(
                                new_state.latitude_deg,
                                new_state.longitude_deg,
                                new_state.velocity_north_mps,
                                new_state.velocity_east_mps,
                                new_state.velocity_down_mps,
                            );
                            let cur_vel = (vx as f32, vy as f32, vz as f32);

                            // Compute ECEF acceleration from velocity delta
                            if let Some(prev) = prev_ecef_vel
                                && dt_sec > 0.0
                            {
                                let dt = dt_sec as f32;
                                new_state.accel_x = (cur_vel.0 - prev.0) / dt;
                                new_state.accel_y = (cur_vel.1 - prev.1) / dt;
                                new_state.accel_z = (cur_vel.2 - prev.2) / dt;
                            }
                            *prev_ecef_vel = Some(cur_vel);


                            let marking = std::mem::take(&mut state.marking);
                            let is_static = state.is_static_entity;
                            let manual = state.manual_override;
                            let has_waypoints = state.has_waypoints;
                            let alt_offset = state.manual_alt_offset_m;
                            let fg_ail = state.fg_aileron;
                            let fg_ele = state.fg_elevator;
                            let fg_rud = state.fg_rudder;
                            let fg_thr = state.fg_throttle;
                            let fg_trim = state.fg_elevator_trim;
                            let waypoints_copy = std::mem::take(&mut state.waypoints);
                            *state = new_state;
                            state.marking = marking;
                            state.is_static_entity = is_static;
                            state.manual_override = manual;
                            state.has_waypoints = has_waypoints;
                            state.manual_alt_offset_m = alt_offset;
                            state.fg_aileron = fg_ail;
                            state.fg_elevator = fg_ele;
                            state.fg_rudder = fg_rud;
                            state.fg_throttle = fg_thr;
                            state.fg_elevator_trim = fg_trim;
                            state.waypoints = waypoints_copy;
                        }
                        Err(e) => {
                            error!(entity_id, tick, error = %e, "fdm.read_state failed");
                            *status = EntityStatus::Dead;
                            counter!("supercell_fdm_errors_total", "operation" => "read").increment(1);
                            return;
                        }
                    }

                    // ── FlightGear bridge: update interp thread state ──
                    if bridge.is_some()
                        && let Ok(mut lock) = fg_state_writer.lock()
                    {
                            // Capture convergence snapshot: where the entity
                            // visually is RIGHT NOW (DR-extrapolated from the
                            // previous state) before we replace the state.
                            let converge = lock.as_ref().map(|prev| {
                                let dt = prev.timestamp.elapsed().as_secs_f64();
                                let visual = extrapolate_fg_state(&prev.state, dt);
                                FgConvergeSnapshot {
                                    latitude_deg: visual.latitude_deg,
                                    longitude_deg: visual.longitude_deg,
                                    altitude_m: visual.altitude_m,
                                    altitude_msl_m: visual.altitude_msl_m,
                                    roll_deg: visual.roll_deg,
                                    pitch_deg: visual.pitch_deg,
                                    yaw_deg: visual.yaw_deg,
                                }
                            });
                            *lock = Some(FgInterpState {
                                state: state.clone(),
                                timestamp: Instant::now(),
                                converge,
                            });
                    }

                    // ── Waypoint arrival check (loop) ──
                    if !waypoints.is_empty() {
                        let wp = &waypoints[*active_wp];
                        let horizontal_m = haversine_distance_m(
                            state.latitude_deg,
                            state.longitude_deg,
                            wp.latitude_deg,
                            wp.longitude_deg,
                        );
                        let vertical_m = (state.altitude_msl_m - wp.altitude_m).abs();
                        let distance_3d_m = horizontal_m.hypot(vertical_m);
                        if distance_3d_m < waypoint_threshold_m {
                            let prev = *active_wp;
                            *active_wp = (*active_wp + 1) % waypoints.len();
                            counter!("supercell_waypoints_reached_total").increment(1);
                            info!(
                                entity_id,
                                from_wp = prev,
                                to_wp = *active_wp,
                                distance_3d_m = format!("{:.1}", distance_3d_m),
                                waypoint_threshold_m = format!("{:.1}", waypoint_threshold_m),
                                "waypoint.reached"
                            );
                        }
                    }
                }
            });

            // --- Publish DIS ---
            for entity in &mut self.entities {
                if entity.status() != EntityStatus::Active {
                    continue;
                }
                let state = entity.state().clone();
                if let Err(e) = self.dis.publish(&state) {
                    error!(entity_id = state.entity_id, tick, error = %e, "dis.publish failed");
                    counter!("supercell_dis_publish_errors_total").increment(1);
                } else {
                    counter!("supercell_dis_pdus_published_total").increment(1);
                }

                // If this is the ownship (identified by the blue_entity_id),
                // pass its state to the OWP manager.
                if state.entity_id == self.blue_entity_id
                    && let Some(owp) = &self.owp_publisher
                {
                    owp.update_entity_state(state.clone());
                    counter!("supercell_owp_updates_total").increment(1);
                }
            }

            // --- Timing ---
            let elapsed = t_start.elapsed();
            counter!("supercell_ticks_total").increment(1);
            histogram!("supercell_tick_duration_seconds").record(elapsed.as_secs_f64());

            debug!(
                tick,
                elapsed_ms = elapsed.as_secs_f64() * 1000.0,
                "sim.tick"
            );

            if elapsed > tick_duration + tick_duration / 10 {
                warn!(
                    tick,
                    budget_ms = tick_duration.as_secs_f64() * 1000.0,
                    actual_ms = elapsed.as_secs_f64() * 1000.0,
                    "sim.tick overrun"
                );
            } else if let Some(remaining) = tick_duration.checked_sub(elapsed) {
                std::thread::sleep(remaining);
            }
        }

        info!(ticks_completed = tick, "sim.run shutdown");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_state() -> EntityState {
        EntityState {
            entity_id: 1,
            site_id: 1,
            application_id: 1,
            force_id: 1,
            latitude_deg: 0.0,
            longitude_deg: 0.0,
            altitude_m: 1000.0,
            altitude_msl_m: 1000.0,
            ..EntityState::default()
        }
    }

    #[test]
    fn extrapolate_fg_state_keeps_longitude_finite_near_poles() {
        let mut state = sample_state();
        state.latitude_deg = 89.999_999;
        state.velocity_north_mps = 120.0;
        state.velocity_east_mps = 120.0;

        let interp = extrapolate_fg_state(&state, 0.5);

        assert!(
            interp.longitude_deg.is_finite(),
            "longitude must remain finite near poles"
        );
        assert!(
            interp.latitude_deg.is_finite(),
            "latitude must remain finite near poles"
        );
    }

    #[test]
    fn clone_fg_interp_state_returns_err_for_poisoned_mutex() {
        let shared: Arc<Mutex<Option<FgInterpState>>> = Arc::new(Mutex::new(Some(FgInterpState {
            state: sample_state(),
            timestamp: Instant::now(),
            converge: None,
        })));

        let shared_for_panic = Arc::clone(&shared);
        let _ = std::thread::spawn(move || {
            if let Ok(_guard) = shared_for_panic.lock() {
                panic!("poison fg interp mutex");
            }
        })
        .join();

        let snapshot = clone_fg_interp_state(&shared);
        assert!(snapshot.is_err(), "poisoned mutex should return an error");
    }

    #[test]
    fn smoothstep_endpoints_and_midpoint() {
        assert!((smoothstep(0.0) - 0.0).abs() < 1e-12);
        assert!((smoothstep(1.0) - 1.0).abs() < 1e-12);
        assert!((smoothstep(0.5) - 0.5).abs() < 1e-12);
    }

    #[test]
    fn smoothstep_clamps_out_of_range() {
        assert!((smoothstep(-0.5) - 0.0).abs() < 1e-12);
        assert!((smoothstep(1.5) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn smoothstep_is_monotonic() {
        let mut prev = 0.0;
        for i in 0..=100 {
            let t = i as f64 / 100.0;
            let s = smoothstep(t);
            assert!(
                s >= prev,
                "smoothstep must be monotonic: s({t}) = {s} < {prev}"
            );
            prev = s;
        }
    }

    #[test]
    fn convergence_blends_position_at_start_and_end() {
        // At t=0 (start of convergence), output should equal the converge snapshot.
        // At t>=CONVERGE_SECONDS, output should equal the DR-extrapolated state.
        let conv = FgConvergeSnapshot {
            latitude_deg: 50.0,
            longitude_deg: 4.0,
            altitude_m: 1000.0,
            altitude_msl_m: 1000.0,
            roll_deg: 5.0,
            pitch_deg: 2.0,
            yaw_deg: 90.0,
        };

        let mut state = sample_state();
        state.latitude_deg = 50.001;
        state.longitude_deg = 4.001;
        state.altitude_m = 1010.0;
        state.altitude_msl_m = 1010.0;
        state.roll_deg = 10.0;
        state.pitch_deg = 4.0;
        state.yaw_deg = 95.0;
        state.velocity_north_mps = 50.0;

        // At t≈0: smoothstep(0) = 0 → output ≈ converge snapshot
        let dr = extrapolate_fg_state(&state, 0.0);
        let s = smoothstep(0.0);
        let blended_lat = conv.latitude_deg + (dr.latitude_deg - conv.latitude_deg) * s;
        assert!(
            (blended_lat - conv.latitude_deg).abs() < 1e-10,
            "at t=0, blended position should equal converge snapshot"
        );

        // At t=CONVERGE_SECONDS: smoothstep(1) = 1 → output = DR position
        let dt = FG_CONVERGE_SECONDS;
        let dr = extrapolate_fg_state(&state, dt);
        let s = smoothstep(1.0);
        let blended_lat = conv.latitude_deg + (dr.latitude_deg - conv.latitude_deg) * s;
        assert!(
            (blended_lat - dr.latitude_deg).abs() < 1e-10,
            "at t=converge_end, blended position should equal DR position"
        );
    }

    #[test]
    fn test_haversine_distance_m_precision_bounds() {
        // Diametrically opposite points on the equator mathematically yield a distance of
        // pi * EARTH_RADIUS_M, and `a` evaluates to exactly 1.0. With floating-point
        // error, `a` could drift slightly above 1.0, triggering NaN from `.asin()`.
        let distance = haversine_distance_m(0.0, 0.0, 0.0, 180.0);
        assert!(!distance.is_nan(), "distance should not be NaN");

        // Same for poles
        let distance_poles = haversine_distance_m(90.0, 0.0, -90.0, 0.0);
        assert!(!distance_poles.is_nan(), "distance_poles should not be NaN");
    }
}

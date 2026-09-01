//! JSBSim TCP console client and FDM adapter.
//!
//! Implements line-oriented `set`, `get`, and `iterate` command handling with timeout-bound I/O.

use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use tracing::{debug, warn};

use crate::config::{FlyingEntityConfig, JsbsimConnectionMode};
use crate::entity::{DisEntityType, EntityState};

// ─── Constants ───────────────────────────────────────────────────────────────

const FT_TO_M: f64 = 0.3048;
const FPS_TO_MPS: f64 = 0.3048;
const TCP_TIMEOUT: Duration = Duration::from_secs(2);
/// Longer timeout for iterate commands during engine spool (many frames).
const ITERATE_TIMEOUT: Duration = Duration::from_secs(30);
const SIMULATION_HZ: u32 = 400;
const KNOT_TO_MPS: f64 = 0.514_444;
const MEAN_EARTH_RADIUS_M: f64 = 6_371_000.0;

#[cfg(not(test))]
const STEP_SYNC_TIMEOUT: Duration = Duration::from_secs(2);
#[cfg(test)]
const STEP_SYNC_TIMEOUT: Duration = Duration::from_millis(50);

#[derive(Clone, Copy, Debug)]
struct StartupPolicy {
    startup_max_attempts: usize,
    connect_retry_window: Duration,
    connect_retry_interval: Duration,
    connect_cancel_poll_interval: Duration,
    reconnect_wait: Duration,
}

impl StartupPolicy {
    const fn production() -> Self {
        Self {
            startup_max_attempts: 3,
            connect_retry_window: Duration::from_secs(15),
            connect_retry_interval: Duration::from_secs(1),
            connect_cancel_poll_interval: Duration::from_millis(100),
            reconnect_wait: Duration::from_secs(5),
        }
    }
}

// ─── FdmHandle trait ─────────────────────────────────────────────────────────

/// Abstraction over a flight dynamics model connection.
pub trait FdmHandle {
    /// Called once before the tick loop (no-op for Remote connections).
    fn start(&mut self) -> Result<()>;

    /// Advance the simulation by `dt_sec` seconds worth of internal frames.
    fn step(&mut self, dt_sec: f64) -> Result<()>;

    /// Read the current kinematic state from the simulator.
    fn read_state(&mut self) -> Result<EntityState>;

    /// Set a named simulator property to `value`.
    fn set_property(&mut self, name: &str, value: f64) -> Result<()>;

    /// Read a named simulator property.
    fn get_property(&mut self, name: &str) -> Result<f64>;
}

// ─── Deterministic kinematic backend ────────────────────────────────────────

/// Simulation-only point-mass backend for scenario-contract integration.
///
/// It preserves configured speed and altitude while accepting SuperCell's
/// existing heading/altitude setpoint interface. Aerodynamic effects are
/// intentionally outside this backend's scope.
pub struct KinematicHandle {
    state: EntityState,
    speed_mps: f64,
    desired_heading_deg: f64,
    desired_altitude_m: f64,
}

impl KinematicHandle {
    /// Construct deterministic motion from an entity's `Kinematic` config.
    pub fn new(config: &FlyingEntityConfig) -> Result<Self> {
        let JsbsimConnectionMode::Kinematic {
            latitude_deg,
            longitude_deg,
            altitude_m,
            heading_deg,
            speed_kts,
        } = &config.jsbsim
        else {
            bail!(
                "entity {} is not configured for kinematic dynamics",
                config.base.entity_id
            );
        };
        let values = [
            *latitude_deg,
            *longitude_deg,
            *altitude_m,
            *heading_deg,
            *speed_kts,
        ];
        if values.iter().any(|value| !value.is_finite()) {
            bail!(
                "entity {} kinematic initial state must be finite",
                config.base.entity_id
            );
        }
        if !(-90.0..=90.0).contains(latitude_deg)
            || !(-180.0..=180.0).contains(longitude_deg)
            || *speed_kts <= 0.0
        {
            bail!(
                "entity {} has invalid kinematic latitude, longitude, or speed",
                config.base.entity_id
            );
        }

        let normalized_heading = heading_deg.rem_euclid(360.0);
        let speed_mps = speed_kts * KNOT_TO_MPS;
        let heading_rad = normalized_heading.to_radians();
        let state = EntityState {
            latitude_deg: *latitude_deg,
            longitude_deg: *longitude_deg,
            altitude_m: *altitude_m,
            altitude_msl_m: *altitude_m,
            velocity_north_mps: speed_mps * heading_rad.cos(),
            velocity_east_mps: speed_mps * heading_rad.sin(),
            yaw_deg: normalized_heading,
            entity_id: config.base.entity_id,
            site_id: config.base.site_id,
            application_id: config.base.application_id,
            force_id: config.base.force_id,
            entity_type: config.base.entity_type.to_dis_entity_type(),
            vcas_kts: *speed_kts as f32,
            ..EntityState::default()
        };
        Ok(Self {
            state,
            speed_mps,
            desired_heading_deg: normalized_heading,
            desired_altitude_m: *altitude_m,
        })
    }
}

impl FdmHandle for KinematicHandle {
    fn start(&mut self) -> Result<()> {
        Ok(())
    }

    fn step(&mut self, dt_sec: f64) -> Result<()> {
        if !dt_sec.is_finite() || dt_sec <= 0.0 {
            bail!("kinematic step must be a positive finite duration");
        }
        let heading_rad = self.desired_heading_deg.to_radians();
        let angular_distance = self.speed_mps * dt_sec / MEAN_EARTH_RADIUS_M;
        let latitude = self.state.latitude_deg.to_radians();
        let longitude = self.state.longitude_deg.to_radians();
        let next_latitude = (latitude.sin() * angular_distance.cos()
            + latitude.cos() * angular_distance.sin() * heading_rad.cos())
        .asin();
        let next_longitude = longitude
            + (heading_rad.sin() * angular_distance.sin() * latitude.cos())
                .atan2(angular_distance.cos() - latitude.sin() * next_latitude.sin());

        self.state.latitude_deg = next_latitude.to_degrees();
        self.state.longitude_deg = (next_longitude.to_degrees() + 180.0).rem_euclid(360.0) - 180.0;
        self.state.altitude_m = self.desired_altitude_m;
        self.state.altitude_msl_m = self.desired_altitude_m;
        self.state.yaw_deg = self.desired_heading_deg;
        self.state.velocity_north_mps = self.speed_mps * heading_rad.cos();
        self.state.velocity_east_mps = self.speed_mps * heading_rad.sin();
        self.state.velocity_down_mps = 0.0;
        self.state.v_body_u_fps = (self.speed_mps / FPS_TO_MPS) as f32;
        self.state.sim_time_s += dt_sec;
        Ok(())
    }

    fn read_state(&mut self) -> Result<EntityState> {
        Ok(self.state.clone())
    }

    fn set_property(&mut self, name: &str, value: f64) -> Result<()> {
        match name {
            "ap/heading_setpoint" => self.desired_heading_deg = value.rem_euclid(360.0),
            "ap/altitude_setpoint" => {
                self.desired_altitude_m = self.state.terrain_elevation_m + value * FT_TO_M
            }
            // Hold flags and manual controls are accepted to preserve the FDM
            // interface; point-mass motion has no actuator dynamics.
            "ap/heading_hold"
            | "ap/attitude_hold"
            | "ap/altitude_hold"
            | "fcs/throttle-cmd-norm" => {}
            _ => bail!("unsupported kinematic property '{name}'"),
        }
        Ok(())
    }

    fn get_property(&mut self, name: &str) -> Result<f64> {
        match name {
            "simulation/sim-time-sec" => Ok(self.state.sim_time_s),
            "ap/heading_setpoint" => Ok(self.desired_heading_deg),
            "ap/altitude_setpoint" => Ok(self.desired_altitude_m / FT_TO_M),
            _ => bail!("unsupported kinematic property '{name}'"),
        }
    }
}

// ─── JsbsimConnection ───────────────────────────────────────────────────────

/// Low-level TCP connection to a single JSBSim console.
struct JsbsimConnection {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    line_buf: String,
}

fn sleep_with_cancel(
    running: &AtomicBool,
    total: Duration,
    poll_interval: Duration,
    context: &str,
) -> Result<()> {
    let start = Instant::now();
    while start.elapsed() < total {
        if !running.load(Ordering::SeqCst) {
            return Err(anyhow!("{context}: cancelled"));
        }
        let remaining = total.saturating_sub(start.elapsed());
        let chunk = remaining.min(poll_interval);
        std::thread::sleep(chunk);
    }
    Ok(())
}

impl JsbsimConnection {
    fn connect_with_policy(
        address: &str,
        running: &AtomicBool,
        policy: &StartupPolicy,
    ) -> Result<Self> {
        let deadline = Instant::now() + policy.connect_retry_window;
        let stream = loop {
            if !running.load(Ordering::SeqCst) {
                return Err(anyhow!("TCP connect to JSBSim at {address}: cancelled"));
            }

            let resolve_and_connect = || -> Result<TcpStream> {
                let addr = address
                    .to_socket_addrs()
                    .context("resolve JSBSim address")?
                    .next()
                    .context("no JSBSim address resolved")?;

                TcpStream::connect_timeout(&addr, Duration::from_millis(500))
                    .map_err(anyhow::Error::from)
            };

            match resolve_and_connect() {
                Ok(s) => break s,
                Err(e) => {
                    if Instant::now() > deadline {
                        return Err(anyhow!(
                            "TCP connect to JSBSim at {address}: {e} (retries exhausted)"
                        ));
                    }
                    debug!(%address, "JSBSim not ready, retrying...");
                    sleep_with_cancel(
                        running,
                        policy.connect_retry_interval,
                        policy.connect_cancel_poll_interval,
                        "JSBSim connect retry wait",
                    )?;
                }
            }
        };
        stream
            .set_read_timeout(Some(TCP_TIMEOUT))
            .context("set TCP read timeout")?;

        let writer = stream.try_clone().context("clone TCP stream")?;
        let mut conn = Self {
            reader: BufReader::new(stream),
            writer,
            line_buf: String::with_capacity(256),
        };

        // Consume optional initial prompt/banner chatter.
        conn.drain_connect_banner();
        Ok(conn)
    }

    /// Read one meaningful response line, skipping empty lines, prompts, and
    /// connection banner chatter.
    fn read_response(&mut self) -> Result<String> {
        loop {
            self.line_buf.clear();
            let n = self
                .reader
                .read_line(&mut self.line_buf)
                .context("TCP read from JSBSim")?;
            if n == 0 {
                return Err(anyhow!("JSBSim TCP connection closed"));
            }
            let trimmed = self.line_buf.trim();
            if trimmed.is_empty() || trimmed == "JSBSim>" {
                continue;
            }
            if trimmed.starts_with("Connected to JSBSim") {
                continue;
            }
            return Ok(trimmed.to_string());
        }
    }

    /// Drain optional connection banner / prompt lines without requiring them.
    fn drain_connect_banner(&mut self) {
        loop {
            self.line_buf.clear();
            match self.reader.read_line(&mut self.line_buf) {
                Ok(0) => break,
                Ok(_) => {
                    let trimmed = self.line_buf.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    if trimmed == "JSBSim>" {
                        break;
                    }
                    if trimmed.starts_with("Connected to JSBSim") {
                        continue;
                    }
                    // Unknown banner line; stop draining so command path can
                    // process subsequent lines normally.
                    break;
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut => {
                    break;
                }
                Err(e) => {
                    debug!(error = %e, "JSBSim connect-banner drain failed; continuing");
                    break;
                }
            }
        }
    }

    /// Send a command and return the response line.
    fn command(&mut self, cmd: &str) -> Result<String> {
        self.writer
            .write_all(cmd.as_bytes())
            .context("TCP write to JSBSim")?;
        self.read_response()
    }

    /// `get <property>` → parse the value as f64.
    fn get_f64(&mut self, property: &str) -> Result<f64> {
        let resp = self.command(&format!("get {property}\n"))?;
        Self::parse_get_response(property, &resp)
    }

    /// Parse a "property = value" response into f64.
    fn parse_get_response(property: &str, resp: &str) -> Result<f64> {
        let (actual_property, value_str) = resp
            .split_once('=')
            .ok_or_else(|| anyhow!("bad get response for '{property}': {resp}"))?;

        // Some JSBSim builds include an inline prompt prefix, e.g.:
        // "JSBSim> simulation/sim-time-sec = 1.5"
        // Accept both prefixed and non-prefixed variants.
        let mut actual_property = actual_property.trim();
        while let Some(rest) = actual_property.strip_prefix("JSBSim>") {
            actual_property = rest.trim_start();
        }
        // Be tolerant of prompt/banners that prepend text before "JSBSim>".
        if let Some((_, rest)) = actual_property.rsplit_once("JSBSim>") {
            actual_property = rest.trim_start();
        }

        if actual_property != property {
            return Err(anyhow!(
                "property mismatch: expected '{property}', got '{actual_property}' (response: {resp})"
            ));
        }

        let value_str = value_str.trim();
        value_str
            .parse::<f64>()
            .map_err(|e| anyhow!("parse '{property}' value '{value_str}': {e}"))
    }

    /// Pipeline multiple `get` commands: send all at once, then read all responses.
    /// Returns values in the same order as `properties`.
    fn batch_get(&mut self, properties: &[&str]) -> Result<Vec<f64>> {
        // Send all get commands without waiting for responses
        for prop in properties {
            self.writer
                .write_all(format!("get {prop}\n").as_bytes())
                .context("TCP write to JSBSim (batch)")?;
        }
        // Read all responses in order
        let mut values = Vec::with_capacity(properties.len());
        for prop in properties {
            let resp = self.read_response()?;
            values.push(Self::parse_get_response(prop, &resp)?);
        }
        Ok(values)
    }

    /// `set <property> <value>` → verify "set successful".
    fn set(&mut self, property: &str, value: f64) -> Result<()> {
        let resp = self.command(&format!("set {property} {value}\n"))?;
        if !resp.ends_with("set successful") {
            return Err(anyhow!("set '{property}' failed: {resp}"));
        }
        Ok(())
    }

    /// `iterate <n>` → verify "Iterations performed".
    fn iterate(&mut self, frames: i32) -> Result<()> {
        let resp = self.command(&format!("iterate {frames}\n"))?;
        if !resp.ends_with("Iterations performed") {
            return Err(anyhow!("iterate {frames} failed: {resp}"));
        }
        Ok(())
    }

    /// Like `iterate`, but temporarily raises the read timeout to
    /// `ITERATE_TIMEOUT` for long-running spool/trim commands.
    fn iterate_slow(&mut self, frames: i32) -> Result<()> {
        self.reader
            .get_ref()
            .set_read_timeout(Some(ITERATE_TIMEOUT))
            .context("set iterate timeout")?;
        let result = self.iterate(frames);
        let _ = self.reader.get_ref().set_read_timeout(Some(TCP_TIMEOUT));
        result
    }
}

// ─── JsbsimHandle ────────────────────────────────────────────────────────────

/// High-level handle to a JSBSim instance, implementing [`FdmHandle`].
pub struct JsbsimHandle {
    conn: JsbsimConnection,
    entity_id: u16,
    site_id: u16,
    application_id: u16,
    force_id: u8,
    entity_type: DisEntityType,
    has_piston_engine_telemetry: bool,
}

impl JsbsimHandle {
    /// Connect to a JSBSim instance as described by `config`.
    ///
    /// Uses an always-true cancellation flag for callers that do not need startup cancellation.
    pub fn new(config: &FlyingEntityConfig) -> Result<Self> {
        let running = AtomicBool::new(true);
        Self::new_with_running(config, &running)
    }

    /// Connect to a JSBSim instance as described by `config` while honoring
    /// cancellation via `running`.
    pub fn new_with_running(config: &FlyingEntityConfig, running: &AtomicBool) -> Result<Self> {
        Self::new_with_policy(config, running, &StartupPolicy::production())
    }

    fn new_with_policy(
        config: &FlyingEntityConfig,
        running: &AtomicBool,
        policy: &StartupPolicy,
    ) -> Result<Self> {
        let jsbsim_mode = &config.jsbsim;

        let address = match jsbsim_mode {
            JsbsimConnectionMode::Kinematic { .. } => {
                bail!("kinematic dynamics must use KinematicHandle")
            }
            JsbsimConnectionMode::Remote { address } => address.clone(),
            JsbsimConnectionMode::Spawn { port, .. } => {
                // Support Spawn config for backwards compatibility — connect
                // to localhost on the configured port. The actual JSBSim
                // process is started externally (e.g. by compose).
                let p = port.unwrap_or(5556);
                format!("127.0.0.1:{p}")
            }
        };

        let mut last_error: Option<anyhow::Error> = None;

        for attempt in 0..policy.startup_max_attempts {
            if !running.load(Ordering::SeqCst) {
                return Err(anyhow!(
                    "entity {}: startup cancelled before connect attempt",
                    config.base.entity_id
                ));
            }

            debug!(%address, entity_id = config.base.entity_id, attempt, "connecting to JSBSim");

            let conn = match JsbsimConnection::connect_with_policy(&address, running, policy) {
                Ok(conn) => conn,
                Err(e) => {
                    last_error = Some(e.context(format!(
                        "entity {}: JSBSim connect attempt {} failed",
                        config.base.entity_id,
                        attempt + 1
                    )));
                    continue;
                }
            };

            let mut handle = Self {
                conn,
                entity_id: config.base.entity_id,
                site_id: config.base.site_id,
                application_id: config.base.application_id,
                force_id: config.base.force_id,
                entity_type: config.base.entity_type.to_dis_entity_type(),
                has_piston_engine_telemetry: false,
            };

            let startup_result = handle.run_startup().with_context(|| {
                format!("entity {} startup sequence failed", config.base.entity_id)
            });

            let post_trim_result = match startup_result {
                Ok(()) => handle
                    .conn
                    .get_f64("simulation/sim-time-sec")
                    .with_context(|| {
                        format!("entity {} post-trim health check", config.base.entity_id)
                    })
                    .map(|_| ()),
                Err(e) => Err(e),
            };

            match post_trim_result {
                Ok(()) => {
                    // Engine instrumentation is model-specific. Probe once at
                    // startup so fighter models do not poison the hot-path
                    // state batch with C172-only property requests.
                    let piston_engine_properties = [
                        "propulsion/engine[0]/propeller-rpm",
                        "propulsion/engine[0]/egt-degF",
                        "propulsion/engine[0]/cht-degF",
                        "propulsion/engine[0]/oil-temperature-degF",
                        "propulsion/engine[0]/oil-pressure-psi",
                        "propulsion/engine[0]/fuel-flow-rate-gph",
                        "propulsion/engine[0]/map-inhg",
                    ];
                    handle.has_piston_engine_telemetry = piston_engine_properties
                        .iter()
                        .all(|property| handle.conn.get_f64(property).is_ok());
                    match handle.conn.get_f64("simulation/dt") {
                        Ok(dt) => {
                            let actual_hz = 1.0 / dt;
                            debug!(
                                entity_id = config.base.entity_id,
                                jsbsim_dt = format!("{dt:.6}"),
                                jsbsim_hz = format!("{actual_hz:.1}"),
                                expected_hz = SIMULATION_HZ,
                                "JSBSim dt check"
                            );
                        }
                        Err(e) => {
                            warn!(entity_id = config.base.entity_id, error = %e, "could not read simulation/dt");
                        }
                    }
                    debug!(
                        entity_id = config.base.entity_id,
                        %address,
                        attempt,
                        "JSBSim connected"
                    );
                    return Ok(handle);
                }
                Err(e) => {
                    warn!(
                        entity_id = config.base.entity_id,
                        attempt,
                        error = %e,
                        "JSBSim startup attempt failed"
                    );
                    last_error = Some(e);
                    if attempt < policy.startup_max_attempts - 1 {
                        sleep_with_cancel(
                            running,
                            policy.reconnect_wait,
                            policy.connect_cancel_poll_interval,
                            "JSBSim reconnect wait",
                        )?;
                    }
                }
            }
        }

        match last_error {
            Some(e) => Err(e.context(format!(
                "entity {}: JSBSim startup failed after {} attempts",
                config.base.entity_id, policy.startup_max_attempts
            ))),
            None => Err(anyhow!(
                "entity {}: JSBSim startup failed without an error",
                config.base.entity_id
            )),
        }
    }

    /// Start the engine, apply known-good trim values, and settle.
    ///
    /// Avoids `simulation/do_simple_trim` because the JSBSim trim solver can
    /// diverge nondeterministically and terminates the process on failure.
    /// Instead we set the C172 trim state directly (AoA ≈ 1.39°, throttle 0.70,
    /// pitch trim 0.19 — values captured from successful solver runs at
    /// 4000 ft / 90 KCAS) and iterate to let the FDM settle.
    fn run_startup(&mut self) -> Result<()> {
        // Engine start
        let start_props: &[(&str, f64)] = &[
            ("propulsion/magneto_cmd", 3.0),
            ("propulsion/starter_cmd", 1.0),
            ("fcs/throttle-cmd-norm[0]", 0.70),
            ("fcs/throttle-cmd-norm", 0.70),
            ("fcs/mixture-cmd-norm", 0.87),
        ];
        for &(name, value) in start_props {
            if let Err(e) = self.conn.set(name, value) {
                debug!(property = name, error = %e, "startup property ignored");
            }
        }

        // Spool the engine
        self.conn
            .iterate_slow(200)
            .context("startup: engine spool")?;

        // Apply known-good trim state for C172 at 4000 ft / 90 KCAS
        let trim_props: &[(&str, f64)] = &[("fcs/pitch-trim-cmd-norm", 0.19)];
        for &(name, value) in trim_props {
            if let Err(e) = self.conn.set(name, value) {
                debug!(property = name, error = %e, "trim property ignored");
            }
        }

        // Settle: let the FDM integrate with trim applied
        self.conn
            .iterate_slow(100)
            .context("startup: post-trim settle")?;

        debug!(
            entity_id = self.entity_id,
            "startup: engine started and trim applied"
        );
        Ok(())
    }
}

impl FdmHandle for JsbsimHandle {
    fn start(&mut self) -> Result<()> {
        debug!(entity_id = self.entity_id, "fdm.started (iterate-driven)");
        Ok(())
    }

    fn step(&mut self, dt_sec: f64) -> Result<()> {
        let total = (dt_sec * SIMULATION_HZ as f64).round() as i32;
        let total = total.max(1);

        let t_start = self.conn.get_f64("simulation/sim-time-sec")?;
        let target_time = t_start + (total as f64) / (SIMULATION_HZ as f64);

        self.conn
            .iterate(total)
            .with_context(|| format!("entity {} iterate({})", self.entity_id, total))?;

        let deadline = Instant::now() + STEP_SYNC_TIMEOUT;
        loop {
            let t_curr = self.conn.get_f64("simulation/sim-time-sec")?;
            if t_curr >= target_time - 0.0001 {
                break;
            }
            if Instant::now() > deadline {
                return Err(anyhow!(
                    "timeout waiting for JSBSim to integrate (target={}, current={})",
                    target_time,
                    t_curr
                ));
            }
            std::thread::sleep(Duration::from_millis(5));
        }

        debug!(entity_id = self.entity_id, total, "fdm.step");
        Ok(())
    }

    fn read_state(&mut self) -> Result<EntityState> {
        // Pipeline all property reads in a single batch for minimal TCP latency.
        // Order must match the index constants below.
        let props = &[
            "position/lat-geod-rad",             // 0
            "position/long-gc-rad",              // 1
            "position/geod-alt-ft",              // 2  HAE
            "position/h-sl-ft",                  // 3  MSL
            "position/terrain-elevation-asl-ft", // 4
            "velocities/v-north-fps",            // 5
            "velocities/v-east-fps",             // 6
            "velocities/v-down-fps",             // 7
            "attitude/phi-rad",                  // 8  roll
            "attitude/theta-rad",                // 9  pitch
            "attitude/psi-rad",                  // 10 yaw
            "velocities/p-rad_sec",              // 11 roll rate
            "velocities/q-rad_sec",              // 12 pitch rate
            "velocities/r-rad_sec",              // 13 yaw rate
            "aero/alpha-deg",                    // 14 angle of attack
            "aero/beta-deg",                     // 15 sideslip
            "velocities/u-fps",                  // 16 body-frame forward
            "velocities/v-fps",                  // 17 body-frame right
            "velocities/w-fps",                  // 18 body-frame down
            "accelerations/a-pilot-x-ft_sec2",   // 19
            "accelerations/a-pilot-y-ft_sec2",   // 20
            "accelerations/a-pilot-z-ft_sec2",   // 21
            "aero/stall-hyst-norm",              // 22
            "velocities/vc-kts",                 // 23 calibrated airspeed
            "fcs/elevator-pos-norm",             // 24
            "fcs/left-aileron-pos-norm",         // 25
            "fcs/right-aileron-pos-norm",        // 26
            "fcs/rudder-pos-norm",               // 27
            "fcs/pitch-trim-cmd-norm",           // 28
            "fcs/flap-pos-norm",                 // 29
            "gear/gear-pos-norm",                // 30
        ];
        let v = self.conn.batch_get(props).context("read_state batch_get")?;
        let engine = if self.has_piston_engine_telemetry {
            self.conn
                .batch_get(&[
                    "propulsion/engine[0]/propeller-rpm",
                    "propulsion/engine[0]/egt-degF",
                    "propulsion/engine[0]/cht-degF",
                    "propulsion/engine[0]/oil-temperature-degF",
                    "propulsion/engine[0]/oil-pressure-psi",
                    "propulsion/engine[0]/fuel-flow-rate-gph",
                    "propulsion/engine[0]/map-inhg",
                ])
                .context("read_state piston engine telemetry")?
        } else {
            vec![0.0; 7]
        };

        let latitude_deg = v[0].to_degrees();
        let longitude_deg = v[1].to_degrees();
        let altitude_m = v[2] * FT_TO_M;
        let altitude_msl_m = v[3] * FT_TO_M;
        let terrain_elevation_m = v[4] * FT_TO_M;

        let state = EntityState {
            latitude_deg,
            longitude_deg,
            altitude_m,
            altitude_msl_m,
            terrain_elevation_m,
            velocity_north_mps: v[5] * FPS_TO_MPS,
            velocity_east_mps: v[6] * FPS_TO_MPS,
            velocity_down_mps: v[7] * FPS_TO_MPS,
            acceleration_north_mps2: 0.0,
            acceleration_east_mps2: 0.0,
            acceleration_down_mps2: 0.0,
            roll_deg: v[8].to_degrees(),
            pitch_deg: v[9].to_degrees(),
            yaw_deg: v[10].to_degrees(),
            entity_id: self.entity_id,
            site_id: self.site_id,
            application_id: self.application_id,
            force_id: self.force_id,
            entity_type: self.entity_type,
            marking: String::new(), // set by caller
            has_waypoints: false,   // overridden by caller
            roll_rate_rps: v[11],
            pitch_rate_rps: v[12],
            yaw_rate_rps: v[13],
            accel_x: 0.0, // computed by sim loop from velocity delta
            accel_y: 0.0,
            accel_z: 0.0,
            sim_time_s: 0.0, // no longer read (causes frame drift in batch)
            // Engine data from JSBSim
            engine_rpm: engine[0] as f32,
            engine_egt_degf: engine[1] as f32,
            engine_cht_degf: engine[2] as f32,
            engine_oil_temp_degf: engine[3] as f32,
            engine_oil_press_psi: engine[4] as f32,
            engine_fuel_flow_gph: engine[5] as f32,
            engine_mp_inhg: engine[6] as f32,
            // Aero / FCS state
            alpha_deg: v[14] as f32,
            beta_deg: v[15] as f32,
            v_body_u_fps: v[16] as f32,
            v_body_v_fps: v[17] as f32,
            v_body_w_fps: v[18] as f32,
            a_x_pilot_fpss: v[19] as f32,
            a_y_pilot_fpss: v[20] as f32,
            a_z_pilot_fpss: v[21] as f32,
            stall_warning: v[22] as f32,
            vcas_kts: v[23] as f32,
            elevator_pos_norm: v[24] as f32,
            left_aileron_pos_norm: v[25] as f32,
            right_aileron_pos_norm: v[26] as f32,
            rudder_pos_norm: v[27] as f32,
            elevator_trim_norm: v[28] as f32,
            flap_pos_norm: v[29] as f32,
            gear_pos_norm: v[30] as f32,
            is_static_entity: false,
            manual_override: false,
            manual_alt_offset_m: 0.0,
            fg_aileron: 0.0,
            fg_elevator: 0.0,
            fg_rudder: 0.0,
            fg_throttle: 0.5,
            fg_elevator_trim: 0.0,
            waypoints: Vec::new(),
        };

        debug!(
            entity_id = self.entity_id,
            lat = state.latitude_deg,
            lon = state.longitude_deg,
            alt_m = state.altitude_m,
            "fdm.read_state"
        );

        Ok(state)
    }

    fn set_property(&mut self, name: &str, value: f64) -> Result<()> {
        self.conn
            .set(name, value)
            .with_context(|| format!("entity {} set '{name}'", self.entity_id))
    }

    fn get_property(&mut self, name: &str) -> Result<f64> {
        self.conn
            .get_f64(name)
            .with_context(|| format!("entity {} get '{name}'", self.entity_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::config::{EntityBaseConfig, EntityTypeConfig, FlyingEntityConfig};

    #[derive(Clone, Copy)]
    enum ServerBehavior {
        /// All commands succeed normally.
        Normal,
        /// First iterate fails, rest succeed.
        SpoolFailsOnce,
        /// Simulate a stall where sim time never advances.
        StalledSimTime,
    }

    fn spawn_fake_jsbsim_server(behavior: ServerBehavior) -> (String, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake jsbsim listener");
        let addr = listener
            .local_addr()
            .expect("listener local addr")
            .to_string();
        let iterate_calls = Arc::new(AtomicUsize::new(0));
        let iterate_calls_thread = Arc::clone(&iterate_calls);
        // Shared mock simulation time
        let sim_time = Arc::new(AtomicUsize::new(12_000)); // 12.000 seconds in ms
        let sim_time_thread = Arc::clone(&sim_time);

        std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            if stream.write_all(b"JSBSim>\n").is_err() {
                return;
            }
            if stream.flush().is_err() {
                return;
            }

            let Ok(reader_stream) = stream.try_clone() else {
                return;
            };
            let mut reader = BufReader::new(reader_stream);
            let mut line = String::new();

            loop {
                line.clear();
                let Ok(read) = reader.read_line(&mut line) else {
                    break;
                };
                if read == 0 {
                    break;
                }
                let cmd = line.trim();
                if cmd.is_empty() {
                    continue;
                }

                let response = if let Some(property) = cmd.strip_prefix("get ") {
                    match property {
                        "simulation/sim-time-sec" => {
                            let ms = sim_time_thread.load(Ordering::SeqCst);
                            #[allow(clippy::cast_precision_loss)]
                            let sec = ms as f64 / 1000.0;
                            format!("simulation/sim-time-sec = {sec}\n")
                        }
                        "simulation/dt" => "simulation/dt = 0.0025\n".to_string(),
                        _ => format!("{property} = 0.0\n"),
                    }
                } else if cmd.starts_with("iterate ") {
                    let frames: usize = cmd
                        .strip_prefix("iterate ")
                        .unwrap_or("0")
                        .parse()
                        .unwrap_or(0);

                    let call = iterate_calls_thread.fetch_add(1, Ordering::SeqCst);
                    match behavior {
                        ServerBehavior::SpoolFailsOnce if call == 0 => {
                            "iterate failed\n".to_string()
                        }
                        ServerBehavior::StalledSimTime => {
                            // Do not advance sim_time_thread
                            format!("{frames} Iterations performed\n")
                        }
                        _ => {
                            // Advance simulation time by 2.5ms per frame (400hz)
                            let added_ms = (frames * 5) / 2;
                            sim_time_thread.fetch_add(added_ms, Ordering::SeqCst);

                            format!("{frames} Iterations performed\n")
                        }
                    }
                } else if cmd.starts_with("set ") {
                    "ok set successful\n".to_string()
                } else {
                    "unknown command\n".to_string()
                };

                if stream.write_all(response.as_bytes()).is_err() {
                    break;
                }
                if stream.flush().is_err() {
                    break;
                }
            }
        });

        (addr, iterate_calls)
    }

    fn test_policy(startup_max_attempts: usize) -> StartupPolicy {
        StartupPolicy {
            startup_max_attempts,
            connect_retry_window: Duration::from_millis(150),
            connect_retry_interval: Duration::from_millis(40),
            connect_cancel_poll_interval: Duration::from_millis(10),
            reconnect_wait: Duration::from_millis(5),
        }
    }

    fn remote_flying_config(address: String) -> FlyingEntityConfig {
        FlyingEntityConfig {
            base: EntityBaseConfig {
                entity_id: 42,
                site_id: 1,
                application_id: 1,
                force_id: 1,
                name: "test-flying".to_string(),
                entity_type: EntityTypeConfig::default(),
            },
            aircraft: "c172x".to_string(),
            jsbsim: JsbsimConnectionMode::Remote { address },
            flight_plan: None,
        }
    }

    fn kinematic_flying_config() -> FlyingEntityConfig {
        FlyingEntityConfig {
            base: EntityBaseConfig {
                entity_id: 7,
                site_id: 1,
                application_id: 1,
                force_id: 1,
                name: "kinematic-test".to_string(),
                entity_type: EntityTypeConfig::default(),
            },
            aircraft: "f16".to_string(),
            jsbsim: JsbsimConnectionMode::Kinematic {
                latitude_deg: 35.0,
                longitude_deg: -120.0,
                altitude_m: 9_000.0,
                heading_deg: 0.0,
                speed_kts: 350.0,
            },
            flight_plan: None,
        }
    }

    #[test]
    fn kinematic_backend_preserves_speed_and_accepts_navigation_setpoints() {
        let mut handle = KinematicHandle::new(&kinematic_flying_config())
            .expect("valid kinematic configuration");
        let initial = handle.read_state().expect("initial state");
        handle.step(0.2).expect("northbound step");
        let northbound = handle.read_state().expect("northbound state");
        assert!(northbound.latitude_deg > initial.latitude_deg);
        assert!((northbound.vcas_kts - 350.0).abs() < f32::EPSILON);

        handle
            .set_property("ap/heading_setpoint", 90.0)
            .expect("heading setpoint");
        handle
            .set_property("ap/altitude_setpoint", 30_000.0)
            .expect("altitude setpoint");
        handle.step(0.2).expect("eastbound step");
        let eastbound = handle.read_state().expect("eastbound state");
        assert!(eastbound.longitude_deg > northbound.longitude_deg);
        assert!((eastbound.altitude_m - 9_144.0).abs() < 1e-9);
    }

    #[test]
    fn startup_succeeds_with_normal_server() {
        let (address, iterate_calls) = spawn_fake_jsbsim_server(ServerBehavior::Normal);
        let config = remote_flying_config(address);
        let running = AtomicBool::new(true);

        let handle = JsbsimHandle::new_with_policy(&config, &running, &test_policy(1))
            .expect("startup with normal server should succeed");

        // Engine spool (iterate 200) + settle (iterate 100)
        assert!(
            iterate_calls.load(Ordering::SeqCst) >= 2,
            "startup must call iterate for spool and settle, got {}",
            iterate_calls.load(Ordering::SeqCst),
        );
        assert_eq!(handle.entity_id, 42);
    }

    #[test]
    fn startup_fails_when_spool_fails_and_retries_exhausted() {
        let (address, _iterate_calls) = spawn_fake_jsbsim_server(ServerBehavior::SpoolFailsOnce);
        let config = remote_flying_config(address);
        let running = AtomicBool::new(true);

        // With 1 outer attempt, a spool failure should propagate.
        let result = JsbsimHandle::new_with_policy(&config, &running, &test_policy(1));
        let Err(err) = result else {
            panic!("spool failure must be surfaced as startup error");
        };

        let msg = err.to_string();
        assert!(
            msg.contains("startup failed") || msg.contains("engine spool"),
            "expected startup failure, got: {msg}"
        );
    }

    #[test]
    fn connect_retries_stop_when_cancelled() {
        let running = Arc::new(AtomicBool::new(true));
        let running_for_cancel = Arc::clone(&running);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(60));
            running_for_cancel.store(false, Ordering::SeqCst);
        });

        let result = JsbsimConnection::connect_with_policy(
            "127.0.0.1:9",
            running.as_ref(),
            &StartupPolicy {
                startup_max_attempts: 1,
                connect_retry_window: Duration::from_secs(1),
                connect_retry_interval: Duration::from_millis(250),
                connect_cancel_poll_interval: Duration::from_millis(10),
                reconnect_wait: Duration::from_millis(1),
            },
        );

        let Err(err) = result else {
            panic!("connect should stop when cancelled");
        };

        assert!(
            err.to_string().contains("cancelled"),
            "expected cancellation error, got: {err}"
        );
    }

    #[test]
    fn parse_get_response_accepts_matching_property_name() {
        let value = JsbsimConnection::parse_get_response(
            "position/lat-geod-rad",
            "position/lat-geod-rad = 0.611",
        )
        .expect("matching property should parse");

        assert!((value - 0.611).abs() < 1e-12);
    }

    #[test]
    fn parse_get_response_rejects_property_name_mismatch() {
        let err = JsbsimConnection::parse_get_response(
            "position/lat-geod-rad",
            "position/geod-alt-ft = 5000.0",
        )
        .expect_err("mismatched property must fail");

        let msg = err.to_string();
        assert!(
            msg.contains("property mismatch") && msg.contains("position/geod-alt-ft"),
            "unexpected mismatch error: {msg}"
        );
    }

    #[test]
    fn parse_get_response_accepts_prompt_prefixed_property_name() {
        let value = JsbsimConnection::parse_get_response(
            "simulation/sim-time-sec",
            "JSBSim> simulation/sim-time-sec = 1.50249999999998",
        )
        .expect("prompt-prefixed property should parse");

        assert!((value - 1.502_499_999_999_98).abs() < 1e-12);
    }

    #[test]
    fn tcp_timeout_is_two_seconds() {
        assert_eq!(TCP_TIMEOUT, Duration::from_secs(2));
    }

    #[test]
    fn iterate_timeout_exceeds_tcp_timeout() {
        assert!(ITERATE_TIMEOUT > TCP_TIMEOUT);
    }

    #[test]
    fn step_succeeds_when_time_advances() {
        let (address, _iterate_calls) = spawn_fake_jsbsim_server(ServerBehavior::Normal);
        let config = remote_flying_config(address);
        let running = AtomicBool::new(true);

        let mut handle = JsbsimHandle::new_with_policy(&config, &running, &test_policy(1))
            .expect("startup should succeed");

        handle.step(0.2).expect("step should complete successfully");
    }

    #[test]
    fn step_fails_with_timeout_when_time_stalls() {
        let (address, _iterate_calls) = spawn_fake_jsbsim_server(ServerBehavior::StalledSimTime);
        let config = remote_flying_config(address);
        let running = AtomicBool::new(true);

        let mut handle = JsbsimHandle::new_with_policy(&config, &running, &test_policy(1))
            .expect("startup should succeed");

        let err = handle
            .step(0.2)
            .expect_err("step should timeout when sim time stalls");
        let msg = err.to_string();
        assert!(
            msg.contains("timeout waiting for JSBSim"),
            "expected timeout error, got: {msg}"
        );
    }
}

//! Transport-neutral entity state and type definitions.

/// Lifecycle status of a runtime entity.
///
/// `Active` entities are stepped (Flying) and published every tick.
/// `Dead` entities are skipped entirely — no FDM step and no DIS publish.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityStatus {
    /// Entity is stepped (Flying) and published every tick.
    Active,
    /// Entity is skipped — no FDM step and no DIS publish.
    Dead,
}

/// DIS Entity Type record (IEEE 1278.1 §4.4.1).
///
/// Seven-field tuple that classifies the entity: kind, domain, country,
/// category, subcategory, specific, extra.  Carried verbatim in the
/// EntityStatePdu entity_type field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DisEntityType {
    /// Entity kind (1 = Platform, 2 = Munition, 5 = Cultural Feature, …)
    pub kind: u8,
    /// Domain (1 = Land, 2 = Air, 3 = Surface, …)
    pub domain: u8,
    /// Country code (225 = USA per SISO-REF-010)
    pub country: u16,
    /// Category within kind+domain (e.g. 1 = Fighter/Attack for Air Platform)
    pub category: u8,
    /// Subcategory within category
    pub subcategory: u8,
    /// Specific variant within subcategory
    pub specific: u8,
    /// Extra discrimination
    pub extra: u8,
}

/// SuperCell's neutral kinematic state model.
///
/// All fields use SI units or degrees.
///
/// Deliberately avoids jsbsimrs and dis-rs types so FDM and DIS layers can
/// convert at the boundary.
#[derive(Debug, Clone, Default)]
pub struct EntityState {
    // --- Geodetic position ---
    /// Latitude in degrees (geodetic, WGS-84)
    pub latitude_deg: f64,
    /// Longitude in degrees
    pub longitude_deg: f64,
    /// Altitude above WGS-84 ellipsoid (HAE), metres — used for DIS ECEF position.
    pub altitude_m: f64,
    /// Altitude above mean sea level (MSL), metres — used for FlightGear rendering.
    pub altitude_msl_m: f64,
    /// Terrain elevation above sea level (MSL), metres — used for AGL autopilot setpoint.
    pub terrain_elevation_m: f64,

    // --- NED velocity (metres per second) ---
    /// North velocity in m/s (NED frame).
    pub velocity_north_mps: f64,
    /// East velocity in m/s (NED frame).
    pub velocity_east_mps: f64,
    /// Down velocity in m/s (NED frame).
    pub velocity_down_mps: f64,

    // --- Euler orientation (degrees) ---
    /// Roll (phi)
    pub roll_deg: f64,
    /// Pitch (theta)
    pub pitch_deg: f64,
    /// Yaw / heading (psi)
    pub yaw_deg: f64,

    // --- DIS identification ---
    /// DIS entity ID.
    pub entity_id: u16,
    /// DIS site ID.
    pub site_id: u16,
    /// DIS application ID.
    pub application_id: u16,
    /// DIS force ID (0–3).
    pub force_id: u8,

    // --- DIS entity type ---
    /// DIS entity type classification (seven-field tuple).
    pub entity_type: DisEntityType,

    // --- DIS marking (11-char max, e.g. "Eagle-1") ---
    /// Human-readable marking (max 11 ASCII chars in DIS).
    pub marking: String,

    // --- Body-axis angular velocity (rad/s) for dead reckoning ---
    /// Roll rate in rad/s (body axis).
    pub roll_rate_rps: f64,
    /// Pitch rate in rad/s (body axis).
    pub pitch_rate_rps: f64,
    /// Yaw rate in rad/s (body axis).
    pub yaw_rate_rps: f64,

    // --- ECEF linear acceleration (m/s²) for dead reckoning ---
    /// ECEF X-axis linear acceleration in m/s².
    pub accel_x: f32,
    /// ECEF Y-axis linear acceleration in m/s².
    pub accel_y: f32,
    /// ECEF Z-axis linear acceleration in m/s².
    pub accel_z: f32,

    // --- Engine instrumentation (from JSBSim) ---
    /// Engine RPM.
    pub engine_rpm: f32,
    /// Exhaust gas temperature in °F.
    pub engine_egt_degf: f32,
    /// Cylinder head temperature in °F.
    pub engine_cht_degf: f32,
    /// Oil temperature in °F.
    pub engine_oil_temp_degf: f32,
    /// Oil pressure in PSI.
    pub engine_oil_press_psi: f32,
    /// Fuel flow in gallons per hour.
    pub engine_fuel_flow_gph: f32,
    /// Manifold pressure in inHg.
    pub engine_mp_inhg: f32,

    // --- Aero / FCS state (from JSBSim, for FlightGear FDM) ---
    /// Angle of attack in degrees.
    pub alpha_deg: f32,
    /// Sideslip angle in degrees.
    pub beta_deg: f32,
    /// Body-frame forward velocity in ft/s.
    pub v_body_u_fps: f32,
    /// Body-frame right velocity in ft/s.
    pub v_body_v_fps: f32,
    /// Body-frame down velocity in ft/s.
    pub v_body_w_fps: f32,
    /// Pilot X-axis acceleration in ft/s².
    pub a_x_pilot_fpss: f32,
    /// Pilot Y-axis acceleration in ft/s².
    pub a_y_pilot_fpss: f32,
    /// Pilot Z-axis acceleration in ft/s².
    pub a_z_pilot_fpss: f32,
    /// Stall warning indicator (0.0–1.0).
    pub stall_warning: f32,
    /// Calibrated airspeed in knots.
    pub vcas_kts: f32,
    /// Elevator position normalized (-1 to 1).
    pub elevator_pos_norm: f32,
    /// Left aileron position normalized (-1 to 1).
    pub left_aileron_pos_norm: f32,
    /// Right aileron position normalized (-1 to 1).
    pub right_aileron_pos_norm: f32,
    /// Rudder position normalized (-1 to 1).
    pub rudder_pos_norm: f32,
    /// Elevator trim position normalized (-1 to 1).
    pub elevator_trim_norm: f32,
    /// Flap position normalized (0 to 1).
    pub flap_pos_norm: f32,
    /// Landing gear position normalized (0 to 1).
    pub gear_pos_norm: f32,

    // --- Simulation time ---
    /// Simulation time in seconds.
    pub sim_time_s: f64,

    // --- Dead reckoning classification ---
    /// Whether this entity is statically configured (e.g., `kind = "Fixed"`).
    ///
    /// Static entities always publish DIS dead-reckoning algorithm 1 (Static),
    /// independent of instantaneous velocity fields.
    pub is_static_entity: bool,

    // --- Manual override (FlightGear pilot control) ---
    /// Whether manual override is active (pilot has control via FlightGear).
    pub manual_override: bool,
    /// Whether this entity has loaded waypoints for autopilot navigation.
    pub has_waypoints: bool,
    /// Manual altitude offset from waypoint target (metres MSL, accumulates with inputs).
    pub manual_alt_offset_m: f64,

    // --- Latest FlightGear stick/throttle inputs ---
    /// FlightGear aileron input (-1 to 1).
    pub fg_aileron: f64,
    /// FlightGear elevator input (-1 to 1).
    pub fg_elevator: f64,
    /// FlightGear rudder input (-1 to 1).
    pub fg_rudder: f64,
    /// FlightGear throttle input (0 to 1).
    pub fg_throttle: f64,
    /// FlightGear elevator trim input (-1 to 1).
    pub fg_elevator_trim: f64,

    // --- Mission Routing ---
    /// Waypoints comprising the active route plan.
    pub waypoints: Vec<crate::config::Waypoint>,
}

//! Scenario configuration model and validation.

use std::collections::HashSet;
use std::path::PathBuf;

use anyhow::{Result, bail};
use serde::Deserialize;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::entity::DisEntityType;
use crate::time::{TimeMode, validate_simulation_hz, validate_time_rate};

use sleet_types::uci::v2_5::{ClassificationEnum, OwnerProducerEnum};

/// Top-level configuration loaded from a TOML file.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupercellConfig {
    /// Log format: either "text" or "json".
    #[serde(default = "default_log_format")]
    pub log_format: String,
    /// All simulated entities in the scenario.
    pub entities: EntitiesConfig,
    /// DIS network and exercise settings.
    pub dis: DisConfig,
    /// Legacy simulation tick rate in Hz.
    #[serde(default)]
    pub tick_hz: Option<f64>,
    /// Scenario time and simulation pacing configuration.
    #[serde(default)]
    pub time: Option<TimeConfig>,
    /// Seconds to let the FDM run in trimmed flight before engaging autopilots.
    /// Defaults to 5.0 seconds if omitted.  During this period no FCS commands
    /// are written, allowing JSBSim to stabilise on its initial trim state.
    #[serde(default = "default_settle_secs")]
    pub settle_secs: f64,
    /// Optional FlightGear bridge configuration.
    pub flightgear: Option<FlightGearConfig>,
    /// Optional OMS integration configuration.
    pub oms: Option<OmsConfig>,
    /// Admin HTTP server bind address (e.g. "0.0.0.0:21300").
    /// Required to expose `/health`, `/ready`, `/status`, and Prometheus `/metrics`.
    pub admin_bind_addr: Option<String>,
    /// Waypoint arrival threshold radius in metres.
    ///
    /// Runtime treats waypoints as 3D spheres in MSL-relative space and
    /// advances when `sqrt(horizontal_m^2 + vertical_m^2) < waypoint_threshold_m`.
    /// Defaults to 500.0 metres.
    #[serde(default = "default_waypoint_threshold_m")]
    pub waypoint_threshold_m: f64,
    /// Geoid undulation in metres (N = HAE - MSL).
    /// Config altitudes are assumed MSL; this offset converts to HAE for DIS.
    /// Negative means the geoid is below the ellipsoid (typical in Colorado: ~-16.2m).
    /// Default: 0.0 (no correction — treats config altitudes as HAE).
    #[serde(default)]
    pub geoid_undulation_m: f64,
    /// Log level filter. Supports tracing EnvFilter syntax.
    /// Overridden by RUST_LOG environment variable.
    /// Default: "supercell=info".
    #[serde(default = "default_log_level")]
    pub log_level: String,
    /// OpenTelemetry OTLP endpoint for trace export (requires --features otlp).
    #[serde(default)]
    pub otlp_endpoint: Option<String>,
}

/// Fully resolved scenario-time settings.
#[derive(Debug, Clone)]
pub struct ResolvedTimeConfig {
    /// Scenario clock mode.
    pub mode: TimeMode,
    /// Scenario timestamp at simulation start.
    pub epoch: OffsetDateTime,
    /// Fixed simulation integration frequency in Hz.
    pub simulation_hz: f64,
    /// Optional wall-clock publication limiter in Hz.
    pub max_wall_publish_hz: Option<f64>,
}

/// Scenario time and simulation pacing configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimeConfig {
    /// Scenario clock mode.
    #[serde(default)]
    pub mode: TimeModeConfig,
    /// Scenario seconds per wall second for scaled mode.
    #[serde(default = "default_time_rate")]
    pub rate: f64,
    /// RFC 3339 scenario timestamp at simulation start.
    pub epoch: Option<String>,
    /// Fixed simulation integration frequency in Hz.
    pub simulation_hz: Option<f64>,
    /// Optional wall-clock publication limiter in Hz.
    pub max_wall_publish_hz: Option<f64>,
}

/// Deserializable scenario clock mode.
#[derive(Debug, Clone, Copy, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TimeModeConfig {
    /// Scenario time advances at the same rate as wall time.
    #[default]
    Realtime,
    /// Scenario time advances at a configured multiple of wall time.
    Scaled,
    /// Scenario time advances as fast as simulation computation permits.
    Unpaced,
    /// Scenario time advances only through explicit steps.
    Stepped,
}

fn default_time_rate() -> f64 {
    1.0
}

fn default_log_format() -> String {
    "text".to_string()
}

fn default_log_level() -> String {
    "supercell=info".to_string()
}

fn default_settle_secs() -> f64 {
    5.0
}

fn default_waypoint_threshold_m() -> f64 {
    500.0
}

fn default_fg_fdm_send_port() -> u16 {
    21202
}

fn default_fg_ctrls_recv_port() -> u16 {
    21201
}

impl SupercellConfig {
    /// Validate startup/runtime config contracts that are not fully captured by
    /// TOML type deserialization.
    pub fn validate_runtime_contracts(&self) -> Result<()> {
        validate_log_format(&self.log_format)?;
        if let Some(tick_hz) = self.tick_hz {
            validate_tick_hz(tick_hz)?;
        }
        self.time_settings()?;
        validate_waypoint_threshold_m(self.waypoint_threshold_m)?;
        if let Some(la_cal) = self.oms.as_ref().and_then(|oms| oms.la_cal.as_ref()) {
            la_cal.validate_runtime_contracts()?;
        }
        validate_unique_dis_entity_triplets(&self.entities)
    }

    /// Return the configured LA-CAL settings when present.
    pub fn la_cal_config(&self) -> Option<&LaCalConfig> {
        self.oms.as_ref().and_then(|oms| oms.la_cal.as_ref())
    }

    /// Return resolved scenario-time settings using legacy `tick_hz` fallback.
    pub fn time_settings(&self) -> Result<ResolvedTimeConfig> {
        let simulation_hz = self.simulation_hz()?;
        let time_config = self.time.as_ref();
        let mode_config = time_config
            .map(|config| config.mode)
            .unwrap_or(TimeModeConfig::Realtime);
        let rate = time_config.map_or(1.0, |config| config.rate);
        validate_time_rate(rate)?;

        let mode = match mode_config {
            TimeModeConfig::Realtime => TimeMode::Realtime,
            TimeModeConfig::Scaled => TimeMode::Scaled { rate },
            TimeModeConfig::Unpaced => TimeMode::Unpaced,
            TimeModeConfig::Stepped => TimeMode::Stepped,
        };

        let epoch = match time_config.and_then(|config| config.epoch.as_deref()) {
            Some(epoch) => {
                OffsetDateTime::parse(epoch, &time::format_description::well_known::Rfc3339)
                    .map_err(|e| {
                        anyhow::anyhow!(
                            "invalid time.epoch='{epoch}'; expected RFC 3339 UTC timestamp: {e}"
                        )
                    })?
            }
            None => OffsetDateTime::now_utc(),
        };

        let max_wall_publish_hz = time_config.and_then(|config| config.max_wall_publish_hz);
        if let Some(max_wall_publish_hz) = max_wall_publish_hz {
            validate_wall_publish_hz(max_wall_publish_hz)?;
        }

        Ok(ResolvedTimeConfig {
            mode,
            epoch,
            simulation_hz,
            max_wall_publish_hz,
        })
    }

    /// Return the resolved fixed simulation integration rate.
    pub fn simulation_hz(&self) -> Result<f64> {
        let simulation_hz = self
            .time
            .as_ref()
            .and_then(|time| time.simulation_hz)
            .or(self.tick_hz)
            .ok_or_else(|| {
                anyhow::anyhow!("missing simulation rate; provide tick_hz or time.simulation_hz")
            })?;
        validate_simulation_hz(simulation_hz)?;
        Ok(simulation_hz)
    }
}

/// OMS integration configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OmsConfig {
    /// Optional LA-CAL WebSocket configuration.
    #[serde(rename = "la-cal")]
    pub la_cal: Option<LaCalConfig>,
}

/// LA-CAL WebSocket publishing configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaCalConfig {
    /// WebSocket URL for the Sleet LA-CAL OWP endpoint.
    pub ws_url: String,
    /// Service ID to identify as to the LA-CAL router.
    pub service_id: String,
    /// UUID that identifies SuperCell's ownship as the UCI system.
    pub system_uuid: Option<Uuid>,
    /// UUID that identifies SuperCell's ownship as the UCI subsystem.
    pub subsystem_uuid: Option<Uuid>,
    /// Environment/Namespace UUID for UUIDv5 deterministic generation.
    pub namespace_uuid: Option<Uuid>,
    /// System name for UUIDv5 generation (default: "supercell").
    pub system_name: Option<String>,
    /// Subsystem name for UUIDv5 generation (default: "supercell.platform").
    pub subsystem_name: Option<String>,
    /// Optional mission name. If omitted, checks `MISSION_NAME` env var, then falls back to "mission".
    pub mission_name: Option<String>,
    /// Classification level for outbound UCI messages.
    #[serde(default = "default_classification")]
    pub classification: ClassificationEnum,
    /// Owner/Producer code for outbound UCI messages.
    #[serde(default = "default_owner_producer")]
    pub owner_producer: OwnerProducerEnum,
    /// PositionReport publication rate in Hz.
    pub position_hz: f64,
    /// PositionReportDetailed publication rate in Hz.
    pub prd_hz: f64,
}

fn default_classification() -> ClassificationEnum {
    ClassificationEnum::U
}

fn default_owner_producer() -> OwnerProducerEnum {
    OwnerProducerEnum::Usa
}

impl LaCalConfig {
    /// Validate runtime contracts for LA-CAL configuration values.
    pub fn validate_runtime_contracts(&self) -> Result<()> {
        validate_la_cal_service_id(&self.service_id)?;
        validate_la_cal_publish_rate("position_hz", self.position_hz)?;
        validate_la_cal_publish_rate("prd_hz", self.prd_hz)
    }

    /// Resolve `SystemID`, `SubsystemID`, and `MissionID` using explicit overrides or UUIDv5 generation.
    pub fn resolve_uuids(&self) -> Result<(Uuid, Uuid, Uuid)> {
        let default_namespace = Uuid::parse_str("507e1ce1-1111-4444-8888-507e1ce11111").unwrap();

        let namespace = if let Ok(s) = std::env::var("NAMESPACE_UUID") {
            s.parse::<Uuid>().map_err(|e| {
                anyhow::anyhow!(
                    "NAMESPACE_UUID environment variable must be a valid UUID string: {}",
                    e
                )
            })?
        } else {
            self.namespace_uuid.unwrap_or(default_namespace)
        };

        let system_uuid = self.system_uuid.unwrap_or_else(|| {
            let system_name = self.system_name.as_deref().unwrap_or("supercell");
            Uuid::new_v5(&namespace, system_name.as_bytes())
        });

        let subsystem_uuid = self.subsystem_uuid.unwrap_or_else(|| {
            let subsystem_name = self
                .subsystem_name
                .as_deref()
                .unwrap_or("supercell.platform");
            Uuid::new_v5(&namespace, subsystem_name.as_bytes())
        });

        let mission_name = if let Ok(s) = std::env::var("MISSION_NAME") {
            s
        } else {
            self.mission_name
                .clone()
                .unwrap_or_else(|| "mission".to_string())
        };

        let mission_uuid = Uuid::new_v5(&namespace, mission_name.as_bytes());

        Ok((system_uuid, subsystem_uuid, mission_uuid))
    }
}

/// Validate that the LA-CAL service ID is a valid OWP identifier.
pub fn validate_la_cal_service_id(service_id: &str) -> Result<()> {
    if sleet_types::owp::validate_identifier(service_id) {
        return Ok(());
    }

    bail!("invalid oms.la-cal.service_id='{service_id}'; expected a valid OWP identifier")
}

/// Validate that an LA-CAL publish rate is strictly positive.
pub fn validate_la_cal_publish_rate(name: &str, hz: f64) -> Result<()> {
    if hz > 0.0 {
        return Ok(());
    }

    bail!("invalid oms.la-cal.{name}={hz}; expected a positive value")
}

/// Validate that the log format is strictly "text" or "json".
pub fn validate_log_format(format: &str) -> Result<()> {
    if format == "text" || format == "json" {
        return Ok(());
    }

    bail!("invalid log_format='{format}'; expected 'text' or 'json'")
}

/// Validate that simulation tick rate is strictly positive.
pub fn validate_tick_hz(tick_hz: f64) -> Result<()> {
    if tick_hz > 0.0 && tick_hz.is_finite() {
        return Ok(());
    }

    bail!("invalid tick_hz={tick_hz}; expected a positive finite value")
}

/// Validate that the optional wall publish limiter is strictly positive.
pub fn validate_wall_publish_hz(max_wall_publish_hz: f64) -> Result<()> {
    if max_wall_publish_hz > 0.0 && max_wall_publish_hz.is_finite() {
        return Ok(());
    }

    bail!(
        "invalid time.max_wall_publish_hz={max_wall_publish_hz}; expected a positive finite value"
    )
}

/// Validate that waypoint threshold radius is strictly positive.
pub fn validate_waypoint_threshold_m(waypoint_threshold_m: f64) -> Result<()> {
    if waypoint_threshold_m > 0.0 {
        return Ok(());
    }

    bail!("invalid waypoint_threshold_m={waypoint_threshold_m}; expected a positive value")
}

/// Validate that DIS entity identifier triplets are unique across the scenario.
pub fn validate_unique_dis_entity_triplets(entities: &EntitiesConfig) -> Result<()> {
    let mut seen_triplets: HashSet<(u16, u16, u16)> = HashSet::new();

    for entity in entities.iter_all() {
        let base = entity.base();
        let triplet = (base.site_id, base.application_id, base.entity_id);
        if seen_triplets.insert(triplet) {
            continue;
        }

        bail!(
            "duplicate DIS Entity ID ({}, {}, {}) for entity '{}'; expected unique (site_id, application_id, entity_id) triplets",
            base.site_id,
            base.application_id,
            base.entity_id,
            base.name
        );
    }

    Ok(())
}

/// DIS Entity Type record — seven-field classification per IEEE 1278.1.
///
/// All fields default to 0 if omitted in TOML.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct EntityTypeConfig {
    /// Entity kind (1 = Platform, 2 = Munition, 5 = Cultural Feature, …)
    #[serde(default)]
    pub kind: u8,
    /// Domain (1 = Land, 2 = Air, 3 = Surface, …)
    #[serde(default)]
    pub domain: u8,
    /// Country code (225 = USA per SISO-REF-010)
    #[serde(default)]
    pub country: u16,
    /// Category within kind+domain
    #[serde(default)]
    pub category: u8,
    /// Subcategory within category
    #[serde(default)]
    pub subcategory: u8,
    /// Specific variant within subcategory
    #[serde(default)]
    pub specific: u8,
    /// Extra discrimination
    #[serde(default)]
    pub extra: u8,
}

impl EntityTypeConfig {
    /// Convert to the domain-level [`DisEntityType`].
    pub fn to_dis_entity_type(self) -> DisEntityType {
        DisEntityType {
            kind: self.kind,
            domain: self.domain,
            country: self.country,
            category: self.category,
            subcategory: self.subcategory,
            specific: self.specific,
            extra: self.extra,
        }
    }
}

/// Top-level entity container structurally separated by role.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntitiesConfig {
    /// JSBSim flying entity config for the primary blue aircraft.
    pub ownship: FlyingEntityConfig,
    /// JSBSim flying entity config for other aircraft.
    #[serde(default)]
    pub moving: Vec<FlyingEntityConfig>,
    /// Fixed position entities.
    #[serde(default, rename = "static")]
    pub static_: Vec<FixedEntityConfig>,
}

impl EntitiesConfig {
    /// Returns an iterator over all configured entities.
    pub fn iter_all(&self) -> impl Iterator<Item = EntityRef<'_>> {
        let ownship = std::iter::once(EntityRef::Flying(&self.ownship));
        let moving = self.moving.iter().map(EntityRef::Flying);
        let static_ = self.static_.iter().map(EntityRef::Fixed);
        ownship.chain(moving).chain(static_)
    }
}

/// A reference to any entity type in the scenario.
pub enum EntityRef<'a> {
    /// A flying entity config reference.
    Flying(&'a FlyingEntityConfig),
    /// A fixed entity config reference.
    Fixed(&'a FixedEntityConfig),
}

impl<'a> EntityRef<'a> {
    /// Access the base DIS properties common to all entities.
    pub fn base(&self) -> &'a EntityBaseConfig {
        match self {
            Self::Flying(f) => &f.base,
            Self::Fixed(f) => &f.base,
        }
    }
}

/// Common DIS identity fields shared by every entity kind.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntityBaseConfig {
    /// DIS Entity ID — must be unique within the scenario.
    pub entity_id: u16,
    /// DIS Site ID.
    pub site_id: u16,
    /// DIS Application ID.
    pub application_id: u16,
    /// DIS Force ID (0 = Other, 1 = Friendly, 2 = Opposing, 3 = Neutral).
    pub force_id: u8,
    /// Human-readable name for logging/debugging.
    pub name: String,
    /// DIS Entity Type (kind/domain/country/category/subcategory/specific/extra).
    /// Defaults to all zeros if omitted.
    #[serde(default)]
    pub entity_type: EntityTypeConfig,
}

impl EntityBaseConfig {
    /// Validate schema-level DIS contracts for this entity.
    pub fn validate_dis_contracts(&self) -> Result<()> {
        validate_force_id(self.force_id)
    }
}

/// A JSBSim-backed flying aircraft.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlyingEntityConfig {
    /// Common DIS identity properties.
    #[serde(flatten)]
    pub base: EntityBaseConfig,
    /// Aircraft model name passed to JSBSim (e.g. "c172p").
    pub aircraft: String,
    /// How to connect to the JSBSim instance.
    pub jsbsim: JsbsimConnectionMode,
    /// Optional sequence of waypoints used by runtime waypoint-following setpoint control.
    pub flight_plan: Option<Vec<Waypoint>>,
}

/// A fixed ground site that publishes a static EntityStatePdu each tick.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixedEntityConfig {
    /// Common DIS identity properties.
    #[serde(flatten)]
    pub base: EntityBaseConfig,
    /// WGS-84 geodetic latitude in degrees.
    pub latitude_deg: f64,
    /// WGS-84 geodetic longitude in degrees.
    pub longitude_deg: f64,
    /// Altitude above mean sea level in metres.
    pub altitude_m: f64,
}

/// Validate a DIS Force ID against supported enum values.
///
/// SuperCell accepts values 0..=3 (`Other`, `Friendly`, `Opposing`, `Neutral`).
/// Any other value is considered an invalid scenario contract.
pub fn validate_force_id(force_id: u8) -> Result<()> {
    if matches!(force_id, 0..=3) {
        return Ok(());
    }

    bail!(
        "invalid DIS force_id={force_id}; expected one of 0 (Other), 1 (Friendly), 2 (Opposing), 3 (Neutral)"
    )
}

/// A single waypoint in a flight plan.
#[derive(Debug, Clone, Deserialize)]
pub struct Waypoint {
    /// WGS-84 geodetic latitude in degrees.
    pub latitude_deg: f64,
    /// WGS-84 geodetic longitude in degrees.
    pub longitude_deg: f64,
    /// Altitude above mean sea level in metres.
    pub altitude_m: f64,
}

/// Selects between localhost compatibility mode and explicit remote
/// connection mode for the JSBSim TCP console.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum JsbsimConnectionMode {
    /// Compatibility mode: connect to a JSBSim TCP console on localhost.
    ///
    /// `jsbsim_root` is retained for config compatibility and is not consumed
    /// by the runtime connection path.
    Spawn {
        /// Optional JSBSim root path retained for config compatibility.
        jsbsim_root: Option<PathBuf>,
        /// TCP port on `127.0.0.1` for the JSBSim console.
        /// Defaults to 5556.
        port: Option<u16>,
    },
    /// Connect to an already-running JSBSim TCP console server.
    Remote {
        /// Host and port, e.g. `"127.0.0.1:5556"`.
        address: String,
    },
}

/// FlightGear bridge configuration — optional; omit to disable the bridge.
#[derive(Debug, Clone, Deserialize)]
pub struct FlightGearConfig {
    /// UDP address to send FGNetFDM packets to (e.g. "127.0.0.1").
    pub fdm_send_addr: String,
    /// UDP port to send FGNetFDM packets to.
    #[serde(default = "default_fg_fdm_send_port")]
    pub fdm_send_port: u16,
    /// Local address to bind for receiving FGNetCtrls packets (e.g. "0.0.0.0").
    pub ctrls_recv_addr: String,
    /// Local UDP port to receive FGNetCtrls packets on.
    #[serde(default = "default_fg_ctrls_recv_port")]
    pub ctrls_recv_port: u16,
    /// Manual override aggression factor (1–10).
    /// 1 = gentle (slow heading/altitude response), 10 = aggressive (fast response).
    /// Default: 5.
    #[serde(default = "default_override_aggression")]
    pub override_aggression: u8,
    /// Throttle threshold used for manual-override mode transitions.
    ///
    /// Runtime engages manual override when throttle is greater than this value
    /// and disengages when throttle is less than this value.
    /// Default: 0.05.
    #[serde(default = "default_autopilot_threshold")]
    pub autopilot_threshold: f64,
    /// Maximum age in seconds of the most recent valid FG controls packet while
    /// manual override stays active.
    ///
    /// Runtime disengages manual override after this timeout expires with no
    /// fresh controls packet. `0.0` means manual override disengages on the
    /// first tick that does not receive controls.
    /// Default: 1.0.
    #[serde(default = "default_override_timeout_secs")]
    pub override_timeout_secs: f64,
}

fn default_override_aggression() -> u8 {
    5
}

fn default_autopilot_threshold() -> f64 {
    0.05
}

fn default_override_timeout_secs() -> f64 {
    1.0
}

impl FlightGearConfig {
    /// Validate runtime contracts for FlightGear configuration values.
    pub fn validate_runtime_contracts(&self) -> Result<()> {
        validate_override_timeout_secs(self.override_timeout_secs)
    }
}

/// Validate that the FlightGear manual-override timeout is non-negative.
///
/// `0.0` is supported and means immediate disengage on the first tick without
/// controls data.
pub fn validate_override_timeout_secs(timeout_secs: f64) -> Result<()> {
    if timeout_secs >= 0.0 {
        return Ok(());
    }

    bail!("invalid flightgear.override_timeout_secs={timeout_secs}; expected a non-negative value")
}

/// DIS network and exercise configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisConfig {
    /// Multicast group address (e.g. `"239.1.2.3"`)
    pub multicast_addr: String,
    /// UDP port
    pub port: u16,
    /// DIS exercise ID (8-bit field in DIS PDU header)
    pub exercise_id: u8,
    /// IP_MULTICAST_TTL socket option; defaults to 1 (link-local)
    pub ttl: Option<u32>,
    /// Local interface address for multicast send (e.g. "127.0.0.1" for loopback,
    /// "0.0.0.0" or omit for OS default). Useful for local testing where
    /// multicast loopback only works on the loopback interface.
    pub multicast_iface: Option<String>,
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_log_format_accepts_valid() {
        assert!(validate_log_format("text").is_ok());
        assert!(validate_log_format("json").is_ok());
    }

    #[test]
    fn validate_log_format_rejects_invalid() {
        let err = validate_log_format("yaml").unwrap_err();
        assert_eq!(
            err.to_string(),
            "invalid log_format='yaml'; expected 'text' or 'json'"
        );
    }

    #[test]
    fn default_log_format_is_text() {
        let toml_str = r#"
            tick_hz = 10.0
            [dis]
            multicast_addr = "239.1.2.3"
            port = 3000
            exercise_id = 1
            [entities.ownship]
            entity_id = 2
            site_id = 1
            application_id = 1
            force_id = 1
            name = "flying1"
            aircraft = "c172x"
            [entities.ownship.jsbsim]
            type = "Remote"
            address = "127.0.0.1:5556"
            [[entities.static]]
            entity_id = 1
            site_id = 1
            application_id = 1
            force_id = 1
            name = "fixed1"
            latitude_deg = 0.0
            longitude_deg = 0.0
            altitude_m = 0.0
        "#;
        let config: SupercellConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.log_format, "text");
        assert!(config.validate_runtime_contracts().is_ok());
    }

    #[test]
    fn parse_json_log_format() {
        let toml_str = r#"
            log_format = "json"
            tick_hz = 10.0
            [dis]
            multicast_addr = "239.1.2.3"
            port = 3000
            exercise_id = 1
            [entities.ownship]
            entity_id = 2
            site_id = 1
            application_id = 1
            force_id = 1
            name = "flying1"
            aircraft = "c172x"
            [entities.ownship.jsbsim]
            type = "Remote"
            address = "127.0.0.1:5556"
            [[entities.static]]
            entity_id = 1
            site_id = 1
            application_id = 1
            force_id = 1
            name = "fixed1"
            latitude_deg = 0.0
            longitude_deg = 0.0
            altitude_m = 0.0
        "#;
        let config: SupercellConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.log_format, "json");
        assert!(config.validate_runtime_contracts().is_ok());
    }

    #[test]
    fn resolve_uuids_uses_defaults() {
        let la_cal = LaCalConfig {
            ws_url: "ws://127.0.0.1:8080/owp".to_string(),
            service_id: "supercell".to_string(),
            system_uuid: None,
            subsystem_uuid: None,
            namespace_uuid: None,
            system_name: None,
            subsystem_name: None,
            mission_name: None,
            classification: ClassificationEnum::U,
            owner_producer: OwnerProducerEnum::Usa,
            position_hz: 10.0,
            prd_hz: 2.0,
        };

        // Instead of unsafe set_var which fails the forbidden unsafe_code lint,
        // we pass the custom value through a helper function or rely on the `namespace_uuid`
        // fallback being used when the env var is not present. But for a unit test of the
        // fallback chain itself, we'll just test that `namespace_uuid` overrides the default
        // when the environment variable is not present.

        let (sys, sub, mis) = la_cal.resolve_uuids().unwrap();

        let default_namespace = Uuid::parse_str("507e1ce1-1111-4444-8888-507e1ce11111").unwrap();
        // Since we can't reliably and safely override the environment in tests due to
        // the forbid(unsafe_code) lint and thread-safety of env::set_var, we verify
        // that if NAMESPACE_UUID is somehow present, we still successfully return UUIDs,
        // and if absent, it falls back to the default.
        // We'll assert they match *either* the environment var override OR the default fallback
        // to avoid flakiness if the env var leaks from the runner.

        let active_namespace = if let Ok(s) = std::env::var("NAMESPACE_UUID") {
            s.parse::<Uuid>().unwrap_or(default_namespace)
        } else {
            default_namespace
        };

        let active_mission = if let Ok(s) = std::env::var("MISSION_NAME") {
            s
        } else {
            "mission".to_string()
        };

        let expected_sys = Uuid::new_v5(&active_namespace, b"supercell");
        let expected_sub = Uuid::new_v5(&active_namespace, b"supercell.platform");
        let expected_mis = Uuid::new_v5(&active_namespace, active_mission.as_bytes());

        assert_eq!(sys, expected_sys);
        assert_eq!(sub, expected_sub);
        assert_eq!(mis, expected_mis);
    }

    #[test]
    fn resolve_uuids_uses_explicit_namespace() {
        let custom_namespace = Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap();
        let la_cal = LaCalConfig {
            ws_url: "ws://127.0.0.1:8080/owp".to_string(),
            service_id: "supercell".to_string(),
            system_uuid: None,
            subsystem_uuid: None,
            namespace_uuid: Some(custom_namespace),
            system_name: Some("test_sys".to_string()),
            subsystem_name: Some("test_sub".to_string()),
            mission_name: Some("test_mission".to_string()),
            classification: ClassificationEnum::U,
            owner_producer: OwnerProducerEnum::Usa,
            position_hz: 10.0,
            prd_hz: 2.0,
        };

        // The namespace_uuid should only apply if NAMESPACE_UUID env var is absent,
        // but since we can't unset it safely in this test environment, we'll just check
        // the resolution result matches whichever takes precedence.

        let active_namespace = if let Ok(s) = std::env::var("NAMESPACE_UUID") {
            s.parse::<Uuid>().unwrap_or(custom_namespace)
        } else {
            custom_namespace
        };

        let active_mission = if let Ok(s) = std::env::var("MISSION_NAME") {
            s
        } else {
            "test_mission".to_string()
        };

        let (sys, sub, mis) = la_cal.resolve_uuids().unwrap();

        let expected_sys = Uuid::new_v5(&active_namespace, b"test_sys");
        let expected_sub = Uuid::new_v5(&active_namespace, b"test_sub");
        let expected_mis = Uuid::new_v5(&active_namespace, active_mission.as_bytes());

        assert_eq!(sys, expected_sys);
        assert_eq!(sub, expected_sub);
        assert_eq!(mis, expected_mis);
    }

    #[test]
    fn resolve_uuids_respects_explicit_overrides() {
        let explicit_sys = Uuid::parse_str("22222222-2222-2222-2222-222222222222").unwrap();
        let explicit_sub = Uuid::parse_str("33333333-3333-3333-3333-333333333333").unwrap();
        let la_cal = LaCalConfig {
            ws_url: "ws://127.0.0.1:8080/owp".to_string(),
            service_id: "supercell".to_string(),
            system_uuid: Some(explicit_sys),
            subsystem_uuid: Some(explicit_sub),
            namespace_uuid: None,
            system_name: None,
            subsystem_name: None,
            mission_name: None,
            classification: ClassificationEnum::U,
            owner_producer: OwnerProducerEnum::Usa,
            position_hz: 10.0,
            prd_hz: 2.0,
        };

        let (sys, sub, _mis) = la_cal.resolve_uuids().unwrap();

        assert_eq!(sys, explicit_sys);
        assert_eq!(sub, explicit_sub);
    }
}

//! Config schema contract tests.
//!
//! Parses representative TOML fixtures and validates deserialization/validation behavior.

use supercell::config::{
    FlightGearConfig, SupercellConfig, validate_force_id, validate_la_cal_publish_rate,
    validate_la_cal_service_id, validate_override_timeout_secs, validate_tick_hz,
    validate_unique_dis_entity_triplets, validate_waypoint_threshold_m,
};

// ── Helper ────────────────────────────────────────────────────────────────────

fn parse(toml: &str) -> SupercellConfig {
    toml::from_str(toml).expect("TOML parse failed")
}

fn parse_err(toml: &str) -> toml::de::Error {
    toml::from_str::<SupercellConfig>(toml).expect_err("expected parse failure but got Ok")
}

// ── Force ID validation ──────────────────────────────────────────────────────

#[test]
fn validate_force_id_rejects_unsupported_values() {
    validate_force_id(0).expect("0 (Other) is valid");
    validate_force_id(1).expect("1 (Friendly) is valid");
    validate_force_id(2).expect("2 (Opposing) is valid");
    validate_force_id(3).expect("3 (Neutral) is valid");

    let err = validate_force_id(9).expect_err("unsupported force ID must fail validation");
    assert!(err.to_string().contains("invalid DIS force_id=9"));
}

// ── Flying entity with flight plan ────────────────────────────────────────────

#[test]
fn flying_entity_flight_plan_fields() {
    let toml = r#"
tick_hz = 10.0

[entities.ownship]
name           = "Test-1"
entity_id      = 1
site_id        = 1
application_id = 1
force_id       = 1
aircraft       = "c172p"

[entities.ownship.jsbsim]
type = "Spawn"

[[entities.ownship.flight_plan]]
latitude_deg  = 36.0
longitude_deg = -115.0
altitude_m    = 5000.0

[[entities.ownship.flight_plan]]
latitude_deg  = 37.0
longitude_deg = -114.0
altitude_m    = 6000.0

[dis]
multicast_addr = "239.1.2.3"
port           = 3000
exercise_id    = 1
"#;

    let cfg = parse(toml);
    let entity = &cfg.entities.ownship;

    assert_eq!(entity.aircraft, "c172p");
    let waypoints = entity
        .flight_plan
        .as_ref()
        .expect("flight_plan should be Some");
    assert_eq!(waypoints.len(), 2, "expected 2 waypoints");
    assert!((waypoints[0].latitude_deg - 36.0).abs() < 1e-9);
    assert!((waypoints[0].longitude_deg - (-115.0)).abs() < 1e-9);
    assert!((waypoints[0].altitude_m - 5000.0).abs() < 1e-9);
    assert!((waypoints[1].latitude_deg - 37.0).abs() < 1e-9);
}

#[test]
fn flying_entity_no_flight_plan_is_none() {
    let toml = r#"
tick_hz = 10.0

[entities.ownship]
name           = "Test-2"
entity_id      = 2
site_id        = 1
application_id = 1
force_id       = 1
aircraft       = "c172p"

[entities.ownship.jsbsim]
type = "Spawn"

[dis]
multicast_addr = "239.1.2.3"
port           = 3000
exercise_id    = 1
"#;

    let cfg = parse(toml);
    assert!(
        cfg.entities.ownship.flight_plan.is_none(),
        "flight_plan should be None when omitted"
    );
}

// ── Fixed entity ──────────────────────────────────────────────────────────────

#[test]
fn fixed_entity_static_position_fields() {
    let toml = r#"
tick_hz = 10.0

[entities.ownship]
name           = "Test-1"
entity_id      = 1
site_id        = 1
application_id = 1
force_id       = 1
aircraft       = "c172p"

[entities.ownship.jsbsim]
type = "Spawn"

[[entities.static]]
name           = "Site-Alpha"
entity_id      = 10
site_id        = 1
application_id = 1
force_id       = 3
entity_type    = { kind = 7, domain = 1, country = 225 }
latitude_deg   = 36.2
longitude_deg  = -115.0
altitude_m     = 620.0

[dis]
multicast_addr = "239.1.2.3"
port           = 3000
exercise_id    = 1
"#;

    let cfg = parse(toml);
    assert_eq!(cfg.entities.iter_all().count(), 2);

    let static_entity = &cfg.entities.static_[0];
    assert!((static_entity.latitude_deg - 36.2).abs() < 1e-9);
    assert!((static_entity.longitude_deg - (-115.0)).abs() < 1e-9);
    assert!((static_entity.altitude_m - 620.0).abs() < 1e-9);
    assert_eq!(static_entity.base.name, "Site-Alpha");
    assert_eq!(static_entity.base.entity_id, 10);
    assert_eq!(static_entity.base.force_id, 3);
}

// ── Parse failure: missing kind tag (No longer applicable since structure implies kind) ──

// ── Parse failure: invalid kind value (No longer applicable since structure implies kind) ──

// ── Parse failure: missing dis section ───────────────────────────────────────

#[test]
fn missing_dis_section_returns_error() {
    let toml = r#"
tick_hz = 10.0

[entities.ownship]
name           = "Site-Alpha"
entity_id      = 1
site_id        = 1
application_id = 1
force_id       = 3
aircraft       = "c172p"

[entities.ownship.jsbsim]
type = "Spawn"
"#;

    parse_err(toml);
}

// ── DIS config fields ─────────────────────────────────────────────────────────

#[test]
fn dis_config_fields_parsed() {
    let toml = r#"
tick_hz = 5.0

[entities.ownship]
name           = "Test-1"
entity_id      = 1
site_id        = 1
application_id = 1
force_id       = 1
aircraft       = "c172p"

[entities.ownship.jsbsim]
type = "Spawn"

[dis]
multicast_addr = "239.0.0.1"
port           = 4000
exercise_id    = 7
ttl            = 4
"#;

    let cfg = parse(toml);
    assert_eq!(cfg.dis.multicast_addr, "239.0.0.1");
    assert_eq!(cfg.dis.port, 4000);
    assert_eq!(cfg.dis.exercise_id, 7);
    assert_eq!(cfg.dis.ttl, Some(4));
    assert_eq!(cfg.tick_hz, 5.0);
}

#[test]
fn dis_exercise_id_above_u8_range_is_rejected_at_parse_time() {
    let toml = r#"
tick_hz = 5.0

[entities.ownship]
name           = "Test-1"
entity_id      = 1
site_id        = 1
application_id = 1
force_id       = 1
aircraft       = "c172p"

[entities.ownship.jsbsim]
type = "Spawn"

[dis]
multicast_addr = "239.0.0.1"
port           = 4000
exercise_id    = 300
"#;

    let err = parse_err(toml);
    assert!(
        err.to_string().contains("out of range") || err.to_string().contains("invalid value"),
        "unexpected parse error: {err}"
    );
}

#[test]
fn supercell_config_rejects_unknown_top_level_fields() {
    let toml = r#"
tick_hz = 5.0
waypoint_threshold = 100.0

[entities.ownship]
name           = "Test-1"
entity_id      = 1
site_id        = 1
application_id = 1
force_id       = 1
aircraft       = "c172p"

[entities.ownship.jsbsim]
type = "Spawn"

[dis]
multicast_addr = "239.0.0.1"
port           = 4000
exercise_id    = 7
"#;

    let err = parse_err(toml);
    assert!(
        err.to_string()
            .contains("unknown field `waypoint_threshold`"),
        "unexpected parse error: {err}"
    );
}

#[test]
fn waypoint_threshold_defaults_to_500_meters() {
    let toml = r#"
tick_hz = 5.0

[entities.ownship]
name           = "Test-1"
entity_id      = 1
site_id        = 1
application_id = 1
force_id       = 1
aircraft       = "c172p"

[entities.ownship.jsbsim]
type = "Spawn"

[dis]
multicast_addr = "239.0.0.1"
port           = 4000
exercise_id    = 7
"#;

    let cfg = parse(toml);
    assert!((cfg.waypoint_threshold_m - 500.0).abs() < 1e-9);
}

#[test]
fn waypoint_threshold_can_be_configured() {
    let toml = r#"
tick_hz = 5.0
waypoint_threshold_m = 725.0

[entities.ownship]
name           = "Test-1"
entity_id      = 1
site_id        = 1
application_id = 1
force_id       = 1
aircraft       = "c172p"

[entities.ownship.jsbsim]
type = "Spawn"

[dis]
multicast_addr = "239.0.0.1"
port           = 4000
exercise_id    = 7
"#;

    let cfg = parse(toml);
    assert!((cfg.waypoint_threshold_m - 725.0).abs() < 1e-9);
}

#[test]
fn validate_tick_hz_rejects_zero_and_negative_values() {
    validate_tick_hz(5.0).expect("positive tick_hz should be valid");

    let zero_err = validate_tick_hz(0.0).expect_err("tick_hz=0.0 must fail");
    assert!(
        zero_err.to_string().contains("invalid tick_hz=0"),
        "unexpected error: {zero_err}"
    );

    let negative_err = validate_tick_hz(-2.0).expect_err("negative tick_hz must fail");
    assert!(
        negative_err.to_string().contains("invalid tick_hz=-2"),
        "unexpected error: {negative_err}"
    );
}

#[test]
fn validate_waypoint_threshold_m_rejects_zero_and_negative_values() {
    validate_waypoint_threshold_m(500.0).expect("positive threshold should be valid");

    let zero_err =
        validate_waypoint_threshold_m(0.0).expect_err("waypoint_threshold_m=0.0 must fail");
    assert!(
        zero_err
            .to_string()
            .contains("invalid waypoint_threshold_m=0"),
        "unexpected error: {zero_err}"
    );

    let negative_err =
        validate_waypoint_threshold_m(-1.0).expect_err("negative waypoint_threshold_m must fail");
    assert!(
        negative_err
            .to_string()
            .contains("invalid waypoint_threshold_m=-1"),
        "unexpected error: {negative_err}"
    );
}

#[test]
fn duplicate_dis_entity_triplet_is_rejected() {
    let toml = r#"
tick_hz = 5.0

[entities.ownship]
name           = "A"
entity_id      = 1
site_id        = 7
application_id = 9
force_id       = 1
aircraft       = "c172p"

[entities.ownship.jsbsim]
type = "Spawn"

[[entities.moving]]
name           = "B"
entity_id      = 1
site_id        = 7
application_id = 9
force_id       = 2
aircraft       = "c172p"

[entities.moving.jsbsim]
type = "Spawn"

[dis]
multicast_addr = "239.0.0.1"
port           = 4000
exercise_id    = 7
"#;

    let cfg = parse(toml);
    let err = validate_unique_dis_entity_triplets(&cfg.entities)
        .expect_err("duplicate DIS triplets must fail validation");
    assert!(
        err.to_string()
            .contains("duplicate DIS Entity ID (7, 9, 1)"),
        "unexpected error: {err}"
    );
}

// ── OMS LA-CAL config ─────────────────────────────────────────────────────────

#[test]
fn oms_la_cal_config_parsed() {
    let toml = r#"
tick_hz = 5.0

[entities.ownship]
name           = "Test-1"
entity_id      = 1
site_id        = 1
application_id = 1
force_id       = 1
aircraft       = "c172p"

[entities.ownship.jsbsim]
type = "Spawn"

[dis]
multicast_addr = "239.0.0.1"
port           = 4000
exercise_id    = 7

[oms.la-cal]
ws_url         = "ws://127.0.0.1:8080/owp"
service_id     = "supercell"
system_uuid  = "00000000-0000-1000-8000-000000000001"
position_hz   = 10.0
prd_hz         = 2.0
"#;

    let cfg = parse(toml);
    let la_cal = cfg
        .la_cal_config()
        .expect("LA-CAL config should be present");
    assert_eq!(la_cal.ws_url, "ws://127.0.0.1:8080/owp");
    assert_eq!(
        la_cal
            .system_uuid
            .expect("system_uuid should parse")
            .to_string(),
        "00000000-0000-1000-8000-000000000001"
    );
    assert!((la_cal.position_hz - 10.0).abs() < 1e-9);
    assert!((la_cal.prd_hz - 2.0).abs() < 1e-9);
}

#[test]
fn oms_la_cal_missing_system_uuid_is_valid_and_ready() {
    let toml = r#"
tick_hz = 5.0

[entities.ownship]
name           = "Test-1"
entity_id      = 1
site_id        = 1
application_id = 1
force_id       = 1
aircraft       = "c172p"

[entities.ownship.jsbsim]
type = "Spawn"

[dis]
multicast_addr = "239.0.0.1"
port           = 4000
exercise_id    = 7

[oms.la-cal]
ws_url       = "ws://127.0.0.1:8080/owp"
service_id   = "supercell"
position_hz = 10.0
prd_hz       = 2.0
"#;

    let cfg = parse(toml);
    cfg.validate_runtime_contracts()
        .expect("missing system_uuid should not fail startup validation");
    let la_cal = cfg
        .la_cal_config()
        .expect("LA-CAL config should be present");
    assert!(la_cal.system_uuid.is_none());
}

#[test]
fn oms_rejects_unknown_nested_fields() {
    let toml = r#"
tick_hz = 5.0

[entities.ownship]
name           = "Test-1"
entity_id      = 1
site_id        = 1
application_id = 1
force_id       = 1
aircraft       = "c172p"

[entities.ownship.jsbsim]
type = "Spawn"

[dis]
multicast_addr = "239.0.0.1"
port           = 4000
exercise_id    = 7

[oms]
unexpected = true
"#;

    let err = parse_err(toml);
    assert!(
        err.to_string().contains("unknown field `unexpected`"),
        "unexpected parse error: {err}"
    );
}

#[test]
fn oms_la_cal_rejects_unknown_fields() {
    let toml = r#"
tick_hz = 5.0

[entities.ownship]
name           = "Test-1"
entity_id      = 1
site_id        = 1
application_id = 1
force_id       = 1
aircraft       = "c172p"

[entities.ownship.jsbsim]
type = "Spawn"

[dis]
multicast_addr = "239.0.0.1"
port           = 4000
exercise_id    = 7

[oms.la-cal]
ws_url       = "ws://127.0.0.1:8080/owp"
service_id   = "supercell"
ownship_uid = "00000000-0000-1000-8000-000000000001"
position_hz = 10.0
prd_hz       = 2.0
"#;

    let err = parse_err(toml);
    assert!(
        err.to_string().contains("unknown field `ownship_uid`"),
        "unexpected parse error: {err}"
    );
}

#[test]
fn validate_la_cal_publish_rate_rejects_zero_and_negative_values() {
    validate_la_cal_publish_rate("position_hz", 1.0).expect("positive LA-CAL rate is valid");

    let zero_err =
        validate_la_cal_publish_rate("position_hz", 0.0).expect_err("position_hz=0.0 must fail");
    assert!(
        zero_err
            .to_string()
            .contains("invalid oms.la-cal.position_hz=0"),
        "unexpected error: {zero_err}"
    );

    let negative_err =
        validate_la_cal_publish_rate("prd_hz", -1.0).expect_err("negative prd_hz must fail");
    assert!(
        negative_err
            .to_string()
            .contains("invalid oms.la-cal.prd_hz=-1"),
        "unexpected error: {negative_err}"
    );
}

#[test]
fn oms_la_cal_invalid_rates_fail_runtime_validation() {
    let toml = r#"
tick_hz = 5.0

[entities.ownship]
name           = "Test-1"
entity_id      = 1
site_id        = 1
application_id = 1
force_id       = 1
aircraft       = "c172p"

[entities.ownship.jsbsim]
type = "Spawn"

[dis]
multicast_addr = "239.0.0.1"
port           = 4000
exercise_id    = 7

[oms.la-cal]
ws_url       = "ws://127.0.0.1:8080/owp"
service_id   = "supercell"
position_hz = 0.0
prd_hz       = 2.0
"#;

    let cfg = parse(toml);
    let err = cfg
        .validate_runtime_contracts()
        .expect_err("non-positive LA-CAL rates must fail validation");
    assert!(
        err.to_string().contains("invalid oms.la-cal.position_hz=0"),
        "unexpected error: {err}"
    );
}

#[test]
fn validate_la_cal_service_id_rejects_invalid_values() {
    validate_la_cal_service_id("supercell-123").expect("valid identifier is accepted");

    let err = validate_la_cal_service_id("invalid space").expect_err("spaces must fail");
    assert!(
        err.to_string()
            .contains("invalid oms.la-cal.service_id='invalid space'"),
        "unexpected error: {err}"
    );
}

#[test]
fn oms_la_cal_invalid_service_id_fails_runtime_validation() {
    let toml = r#"
tick_hz = 5.0

[entities.ownship]
name           = "Test-1"
entity_id      = 1
site_id        = 1
application_id = 1
force_id       = 1
aircraft       = "c172p"

[entities.ownship.jsbsim]
type = "Spawn"

[dis]
multicast_addr = "239.0.0.1"
port           = 4000
exercise_id    = 7

[oms.la-cal]
ws_url       = "ws://127.0.0.1:8080/owp"
service_id   = "bad id!"
position_hz = 10.0
prd_hz       = 2.0
"#;

    let cfg = parse(toml);
    let err = cfg
        .validate_runtime_contracts()
        .expect_err("invalid service_id must fail validation");
    assert!(
        err.to_string()
            .contains("invalid oms.la-cal.service_id='bad id!'"),
        "unexpected error: {err}"
    );
}

// ── FlightGear config ─────────────────────────────────────────────────────────

/// Minimal TOML fixture with [flightgear] section.
const TOML_WITH_FG: &str = r#"
tick_hz = 10.0

[entities.ownship]
name           = "Test-1"
entity_id      = 1
site_id        = 1
application_id = 1
force_id       = 1
aircraft       = "c172p"

[entities.ownship.jsbsim]
type = "Spawn"

[dis]
multicast_addr = "239.1.2.3"
port           = 3000
exercise_id    = 1

[flightgear]
fdm_send_addr   = "127.0.0.1"
fdm_send_port   = 5500
ctrls_recv_addr = "0.0.0.0"
ctrls_recv_port = 5501

"#;

#[test]
fn flightgear_config_parsed() {
    let cfg: SupercellConfig =
        toml::from_str(TOML_WITH_FG).expect("TOML with [flightgear] should parse");

    let fg = cfg.flightgear.expect("flightgear should be Some");
    assert_eq!(fg.fdm_send_addr, "127.0.0.1");
    assert_eq!(fg.fdm_send_port, 5500);
    assert_eq!(fg.ctrls_recv_addr, "0.0.0.0");
    assert_eq!(fg.ctrls_recv_port, 5501);
    assert_eq!(fg.override_aggression, 5, "default override aggression");
    assert!((fg.autopilot_threshold - 0.05).abs() < 1e-9);
    assert!((fg.override_timeout_secs - 1.0).abs() < 1e-9);
}

#[test]
fn flightgear_autopilot_threshold_can_be_configured() {
    let toml = r#"
tick_hz = 10.0

[entities.ownship]
name           = "Test-1"
entity_id      = 1
site_id        = 1
application_id = 1
force_id       = 1
aircraft       = "c172p"

[entities.ownship.jsbsim]
type = "Spawn"

[dis]
multicast_addr = "239.1.2.3"
port           = 3000
exercise_id    = 1

[flightgear]
fdm_send_addr       = "127.0.0.1"
fdm_send_port       = 5500
ctrls_recv_addr     = "0.0.0.0"
ctrls_recv_port     = 5501

autopilot_threshold = 0.15
"#;

    let cfg = parse(toml);
    let fg = cfg.flightgear.expect("flightgear should be Some");
    assert!((fg.autopilot_threshold - 0.15).abs() < 1e-9);
}

#[test]
fn flightgear_override_timeout_can_be_configured() {
    let toml = r#"
tick_hz = 10.0

[entities.ownship]
name           = "Test-1"
entity_id      = 1
site_id        = 1
application_id = 1
force_id       = 1
aircraft       = "c172p"

[entities.ownship.jsbsim]
type = "Spawn"

[dis]
multicast_addr = "239.1.2.3"
port           = 3000
exercise_id    = 1

[flightgear]
fdm_send_addr       = "127.0.0.1"
fdm_send_port       = 5500
ctrls_recv_addr     = "0.0.0.0"
ctrls_recv_port     = 5501

override_timeout_secs = 0.25
"#;

    let cfg = parse(toml);
    let fg = cfg.flightgear.expect("flightgear should be Some");
    assert!((fg.override_timeout_secs - 0.25).abs() < 1e-9);
}

#[test]
fn flightgear_config_omitted_ports_use_defaults() {
    let toml = r#"
tick_hz = 10.0

[entities.ownship]
name           = "Test-1"
entity_id      = 1
site_id        = 1
application_id = 1
force_id       = 1
aircraft       = "c172p"

[entities.ownship.jsbsim]
type = "Spawn"

[dis]
multicast_addr = "239.1.2.3"
port           = 3000
exercise_id    = 1

[flightgear]
fdm_send_addr   = "127.0.0.1"
ctrls_recv_addr = "0.0.0.0"

"#;

    let cfg = parse(toml);
    let fg = cfg.flightgear.expect("flightgear should be Some");
    assert_eq!(fg.fdm_send_addr, "127.0.0.1");
    assert_eq!(fg.fdm_send_port, 21202, "default fdm send port");
    assert_eq!(fg.ctrls_recv_addr, "0.0.0.0");
    assert_eq!(fg.ctrls_recv_port, 21201, "default ctrls recv port");
}

#[test]
fn validate_override_timeout_secs_allows_zero_and_rejects_negative_values() {
    validate_override_timeout_secs(0.0).expect("zero timeout should be valid");
    validate_override_timeout_secs(1.0).expect("positive timeout should be valid");

    let err =
        validate_override_timeout_secs(-0.1).expect_err("negative timeout must fail validation");
    assert!(
        err.to_string()
            .contains("invalid flightgear.override_timeout_secs=-0.1"),
        "unexpected error: {err}"
    );
}

#[test]
fn flightgear_config_absent_yields_none() {
    let toml = r#"
tick_hz = 5.0

[entities.ownship]
name           = "Test-1"
entity_id      = 1
site_id        = 1
application_id = 1
force_id       = 1
aircraft       = "c172p"

[entities.ownship.jsbsim]
type = "Spawn"

[dis]
multicast_addr = "239.0.0.1"
port           = 4000
exercise_id    = 7
"#;
    let cfg = parse(toml);
    assert!(
        cfg.flightgear.is_none(),
        "flightgear should be None when section is absent"
    );
}

// Ensure FlightGearConfig is accessible as a public type from the crate.
fn _assert_fg_config_is_pub(_: FlightGearConfig) {}

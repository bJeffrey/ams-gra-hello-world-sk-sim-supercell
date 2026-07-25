# Configuration

SuperCell is configured with a TOML file provided via:

```bash
supercell --config <path/to/config.toml>
```

## Config file

## Top level

| Key | Type | Required | Default | Description |
|---|---|---|---|---|
| `tick_hz` | float | no | — | Legacy simulation tick rate in Hz. Used as `time.simulation_hz` when `[time]` is absent or omits `simulation_hz`. Must be `> 0.0` when present. |
| `log_format` | string | no | `"text"` | Log output format. Either `"text"` or `"json"`. |
| `log_level` | string | no | `"supercell=info"` | Log level filter. Supports tracing EnvFilter syntax. Overridden by `RUST_LOG` environment variable. |
| `otlp_endpoint` | string | no | — | OpenTelemetry OTLP endpoint for trace export (requires `--features otlp`). |
| `settle_secs` | float | no | `5.0` | Startup settle window where stepping and publishing continue but control writes are suppressed. |
| `waypoint_threshold_m` | float | no | `500.0` | 3D waypoint arrival radius in metres. Must be `> 0.0`. |
| `geoid_undulation_m` | float | no | `0.0` | Geoid undulation (`N = HAE - MSL`) used for MSL→HAE conversion where applicable. |
| `admin_bind_addr` | string | no | `None` | Admin HTTP server bind address. Exposes `/health`, `/ready`, `/status`, and `/metrics`. If unset, admin server does not run. Usually `0.0.0.0:21300` |
| `dis` | table | yes | — | DIS network and exercise settings. |
| `flightgear` | table | no | — | FlightGear bridge settings. Omit to disable bridge. |
| `oms` | table | no | — | OMS integration settings. Omit to disable OMS/LA-CAL setup. |
| `entities` | table | yes | — | Scenario entities structure (ownship, moving, static). |

At least one simulation-rate source is required: either legacy top-level
`tick_hz` or `[time].simulation_hz`. When both are present,
`[time].simulation_hz` wins.

## `[time]` (optional)

| Key | Type | Required | Default | Description |
|---|---|---|---|---|
| `mode` | string | no | `"realtime"` | Scenario clock mode: `"realtime"`, `"scaled"`, `"unpaced"`, or `"stepped"`. |
| `rate` | float | no | `1.0` | Scenario seconds per wall second for scaled mode. Must be positive and finite. |
| `epoch` | RFC 3339 string | no | Current UTC at startup | Scenario timestamp at simulation start. |
| `simulation_hz` | float | no if `tick_hz` is present | `tick_hz` | Fixed simulation integration frequency. Must be positive and finite. |
| `max_wall_publish_hz` | float | no | — | Optional wall-monotonic limit for OWP publication batches. Excess due reports are coalesced without changing scenario time. Must be positive and finite when present. |

## `[dis]`

| Key | Type | Required | Default | Description |
|---|---|---|---|---|
| `multicast_addr` | string | yes | — | Destination IPv4 address for DIS output (multicast or unicast). |
| `port` | integer (`u16`) | yes | — | Destination UDP port. |
| `exercise_id` | integer (`u8`) | yes | — | DIS exercise ID placed in PDU header. |
| `ttl` | integer (`u32`) | no | `1` (socket default in runtime path) | Multicast TTL for outbound traffic. |
| `multicast_iface` | string | no | OS default | Outbound interface address for multicast send. |

## `[oms.la-cal]` (optional)

Presence of this table configures UCI LA-CAL publishing readiness. If present, startup begins maintaining the configured OWP WebSocket connection in a background thread and dynamic `SystemID` / `SubsystemID` UUIDs are resolved or generated via UUIDv5.

| Key | Type | Required | Default | Description |
|---|---|---|---|---|
| `ws_url` | string | yes | — | Plain `ws://` WebSocket URL for the Sleet LA-CAL OWP endpoint. `wss://` is not supported by the current client build. |
| `service_id` | string | yes | — | Service ID to identify as to the LA-CAL router. |
| `system_uuid` | UUID string | no | — | UCI `SystemID` UUID for the ownship entity. If not provided, it is generated via UUIDv5. |
| `subsystem_uuid` | UUID string | no | — | UCI `SubsystemID` UUID for the ownship entity. If not provided, it is generated via UUIDv5. |
| `namespace_uuid` | UUID string | no | `507e...` | Realm UUID used for UUIDv5 deterministic generation. Overridden by `NAMESPACE_UUID` env var. |
| `system_name` | string | no | `"supercell"` | String name used for dynamic UUIDv5 `SystemID` generation. |
| `subsystem_name` | string | no | `"supercell.platform"` | String name used for dynamic UUIDv5 `SubsystemID` generation. |
| `mission_name` | string | no | `"mission"` | String name used for dynamic UUIDv5 `MissionID` generation. Overridden by `MISSION_NAME` env var. |
| `classification` | string | no | `"U"` | Classification marking used in `SecurityInformation`. Examples: `"U"`, `"C"`, `"S"`, `"TS"`. |
| `owner_producer` | string | no | `"USA"` | Owner/producer code used in `SecurityInformation`. Examples: `"USA"`, `"NATO"`, `"FGI"`. |
| `position_hz` | float | yes | — | PositionReportDetailed publish rate in simulated Hz. Must be `> 0.0`. |
| `prd_hz` | float | yes | — | Periodic reporting rate in Hz (e.g. `SystemStatus`, `NavigationReport`). Must be `> 0.0`. |
| `navigation_timing_error_seconds` | float | no | `0.01` | One-sigma EGI timing uncertainty. SuperCell propagates it through NED velocity and acceleration to derive PositionReportDetailed position/velocity covariance. Must be non-negative and finite. |

## `[entities]`

The entities table groups scenario participants by role.

| Key | Type | Required | Default | Description |
|---|---|---|---|---|
| `ownship` | table | yes | — | The primary blue flying entity (identifies the system for FlightGear and OWP). |
| `moving` | array of tables | no | empty | Additional flying entities. |
| `static` | array of tables | no | empty | Fixed-position entities. |

### Flying entity (`[entities.ownship]` and `[[entities.moving]]`)

Common identity keys:
| Key | Type | Required | Default | Description |
|---|---|---|---|---|
| `name` | string | yes | — | Human-readable name and DIS marking source. |
| `entity_id` | integer (`u16`) | yes | — | DIS entity ID. |
| `site_id` | integer (`u16`) | yes | — | DIS site ID. |
| `application_id` | integer (`u16`) | yes | — | DIS application ID. |
| `force_id` | integer (`u8`) | yes | — | Allowed values: `0..=3`. |
| `entity_type` | table | no | all zeros | Optional DIS entity type tuple. |

`entity_type` keys: `kind` (`u8`), `domain` (`u8`), `country` (`u16`), `category` (`u8`), `subcategory` (`u8`), `specific` (`u8`), `extra` (`u8`).

Flying-specific keys:
| Key | Type | Required | Default | Description |
|---|---|---|---|---|
| `aircraft` | string | yes | — | JSBSim aircraft model name. |
| `jsbsim` | table | yes | — | JSBSim connection mode (`Remote` or `Spawn`). |
| `flight_plan` | array of tables | no | — | Waypoints for runtime navigation. |

`jsbsim` modes:
- `type = "Remote"` + `address = "host:port"`
- `type = "Spawn"` + optional `port`, optional `jsbsim_root`

`flight_plan` keys (`[[entities.ownship.flight_plan]]` or `[[entities.moving.flight_plan]]`):
| Key | Type | Required | Description |
|---|---|---|---|
| `latitude_deg` | float | yes | WGS-84 geodetic latitude in degrees. |
| `longitude_deg` | float | yes | WGS-84 geodetic longitude in degrees. |
| `altitude_m` | float | yes | Altitude in metres (MSL). |

### Fixed entity (`[[entities.static]]`)

Contains the same identity keys as flying entities, plus fixed position keys:

| Key | Type | Required | Description |
|---|---|---|---|
| `latitude_deg` | float | yes | WGS-84 geodetic latitude in degrees. |
| `longitude_deg` | float | yes | WGS-84 geodetic longitude in degrees. |
| `altitude_m` | float | yes | Altitude in metres (MSL). |

## `[flightgear]` (optional)

| Key | Type | Required | Default | Description |
|---|---|---|---|---|
| `fdm_send_addr` | string | yes | — | Destination address for `FGNetFDM` output. |
| `fdm_send_port` | integer (`u16`) | no | `21202` | Destination port for `FGNetFDM` output. |
| `ctrls_recv_addr` | string | yes | — | Local bind address for `FGNetCtrls` input. |
| `ctrls_recv_port` | integer (`u16`) | no | `21201` | Local bind port for `FGNetCtrls` input. |
| `override_aggression` | integer (`u8`) | no | `5` | Manual override response tuning. Runtime clamps to `1..=10`. |
| `autopilot_threshold` | float | no | `0.05` | Manual mode engages when throttle exceeds this threshold and disengages when below it. |
| `override_timeout_secs` | float | no | `1.0` | Max controls packet age before manual override disengages. Must be `>= 0.0`. |

## Validation summary

Startup rejects:

- Unknown top-level config keys.
- `log_format` not `"text"` or `"json"`.
- Missing both `tick_hz` and `[time].simulation_hz`.
- `tick_hz <= 0.0` when present.
- `[time].simulation_hz <= 0.0` when present.
- `[time].rate <= 0.0`.
- Invalid `[time].epoch` RFC 3339 timestamp.
- `waypoint_threshold_m <= 0.0`.
- Duplicate `(site_id, application_id, entity_id)` tuples.
- `force_id` outside `0..=3`.
- `flightgear.override_timeout_secs < 0.0` when FlightGear config is present.
- `oms.la-cal.position_hz <= 0.0` or `oms.la-cal.prd_hz <= 0.0` when LA-CAL config is present.

## Environment variables

| Variable | Description |
|---|---|
| `RUST_LOG` | Runtime log filter for `tracing` output. |
| `MISSION_NAME` | Explicit override for the `MissionID` generation name. |
| `NAMESPACE_UUID` | Explicit override for the namespace/realm UUID. |

## CLI flags

| Flag | Description |
|---|---|
| `--config <path>` | Path to scenario TOML file (required). |

## Precedence

- Scenario/runtime settings: `CLI arguments > configuration file values > in-code defaults`.
- Logging filter: `RUST_LOG > in-code default (supercell=info)`. `RUST_LOG` is the only external override for log filtering.

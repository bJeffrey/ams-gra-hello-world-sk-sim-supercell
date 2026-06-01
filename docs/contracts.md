# External Interface Contracts

This document is the authoritative registry of interfaces that cross the SuperCell process boundary.

If an interaction is not listed here, it is undocumented behavior.

Field-level DIS details live in `docs/dis-entity-state-pdu.md`; frame/unit transformation details live in `docs/coordinate-pipeline.md`; JSBSim-to-FGNetFDM property mapping details live in `docs/jsbsim-fdm-mapping.md`.

## Inputs

### 1) CLI invocation (`supercell --config <path>`)
- **Direction:** Input
- **External party:** Operator / launcher (shell, container entrypoint, process supervisor)
- **Transport:** Process argv
- **Schema:**
  - Required flag (in run mode): `--config <path/to/config.toml>`
  - Action flag: `--health-check [PORT]` (runs a local HTTP health check and exits; defaults to reading `SUPERCELL_ADMIN_PORT` from the environment if port is not provided)
- **Validation:**
  - Missing `--config` or missing path argument returns process error and exits.
- **Error handling:**
  - Hard fail at startup.
- **Security:**
  - No authentication; local process invocation trust model.
- **Source:** `src/main.rs::parse_config_arg`

### 2) Scenario configuration file (TOML)
- **Direction:** Input
- **External party:** Operator / mission configuration author
- **Transport:** Local file read
- **Schema (current contract):**
  - Top-level:
    - `log_format: String` (optional, defaults to `"text"`; accepts `"text"` or `"json"`)
    - `log_level: String` (optional, defaults to `"supercell=info"`; supports tracing EnvFilter syntax)
    - `tick_hz: f64` (required)
    - `settle_secs: f64` (optional, defaults to `5.0`; suppresses AP/FCS control writes during startup settle phase while stepping/publish continue)
    - `waypoint_threshold_m: f64` (optional, defaults to `500.0`; waypoint arrival sphere radius in metres)
    - `geoid_undulation_m: f64` (optional, defaults to `0.0`)
    - `admin_bind_addr: String` (optional; required to expose `/health`, `/ready`, `/status`, and Prometheus `/metrics` endpoints)
    - `otlp_endpoint: String` (optional; OpenTelemetry OTLP endpoint for trace export)
    - `dis: DisConfig` (required)
    - `flightgear: FlightGearConfig` (optional)
    - `oms: OmsConfig` (optional)
    - `entities: EntitiesConfig` (required)
  - `DisConfig`:
    - `multicast_addr: String` (required IPv4 text; may be multicast or unicast)
    - `port: u16` (required)
    - `exercise_id: u8` (required; parse rejects values outside 0..=255)
    - `ttl: Option<u32>`
    - `multicast_iface: Option<String>`
  - `EntitiesConfig`:
    - `ownship: FlyingEntityConfig` (required)
    - `moving: [FlyingEntityConfig]` (optional array)
    - `static: [FixedEntityConfig]` (optional array)
  - `EntityBaseConfig` (embedded in all entity configs):
    - `entity_id: u16`, `site_id: u16`, `application_id: u16`, `force_id: u8`, `name: String`
    - `entity_type` optional 7-field tuple (`kind/domain/country/category/subcategory/specific/extra`, defaults all zero)
  - `FlyingEntityConfig`:
    - Embedded `EntityBaseConfig` fields
    - `aircraft: String` (required)
    - `jsbsim: JsbsimConnectionMode` (required)
    - `flight_plan: Option<Vec<Waypoint>>`
  - `Waypoint`:
    - `latitude_deg: f64` (required)
    - `longitude_deg: f64` (required)
    - `altitude_m: f64` (required; interpreted as MSL metres)
  - `FixedEntityConfig`:
    - Embedded `EntityBaseConfig` fields
    - `latitude_deg: f64`, `longitude_deg: f64`, `altitude_m: f64` (all required; `altitude_m` interpreted as MSL metres)
  - `JsbsimConnectionMode`:
    - `type = "Remote"` + `address: String`
    - `type = "Spawn"` + optional `jsbsim_root`, optional `port` (compatibility mode; still connects to localhost TCP)
  - `FlightGearConfig`:
    - `fdm_send_addr: String` (required)
    - `fdm_send_port: u16` (optional, defaults to `21202`)
    - `ctrls_recv_addr: String` (required)
    - `ctrls_recv_port: u16` (optional, defaults to `21201`)
    - `override_aggression: u8` (optional, defaults to `5`, runtime-clamped to `1..=10`)
    - `autopilot_threshold: f64` (optional, defaults to `0.05`; manual engage when throttle `> threshold`, disengage when `< threshold`)
    - `override_timeout_secs: f64` (optional, defaults to `1.0`, must be `>= 0.0`; `0.0` means immediate disengage when a tick has no controls)
  - `OmsConfig`:
    - `la-cal: LaCalConfig` (optional table)
  - `LaCalConfig` (`[oms.la-cal]`):
    - `ws_url: String` (required WebSocket URL text)
    - `service_id: String` (required text)
    - `system_uuid: Option<Uuid>` (optional UUID string explicit override; generated via UUIDv5 otherwise)
    - `subsystem_uuid: Option<Uuid>` (optional UUID string explicit override; generated via UUIDv5 otherwise)
    - `namespace_uuid: Option<Uuid>` (optional UUID string fallback for UUIDv5 generation namespace; overridden by `NAMESPACE_UUID` environment variable)
    - `system_name: Option<String>` (optional string name for UUIDv5 system generation; defaults to `"supercell"`)
    - `subsystem_name: Option<String>` (optional string name for UUIDv5 subsystem generation; defaults to `"supercell.platform"`)
    - `mission_name: Option<String>` (optional string name for UUIDv5 mission generation; falls back to `MISSION_NAME` env var, then defaults to `"mission"`)
    - `classification: ClassificationEnum` (optional UCI classification; defaults to `U`)
    - `owner_producer: OwnerProducerEnum` (optional UCI owner/producer code; defaults to `USA`)
    - `position_hz: f64` (required, must be `> 0.0`)
    - `prd_hz: f64` (required, must be `> 0.0`)
- **Validation:**
  - TOML parse/deserialization must succeed.
  - `SupercellConfig` and nested configuration structs enforce `deny_unknown_fields`:
    - Unknown keys are rejected at parse time.
  - Startup enforces `tick_hz > 0.0`.
  - Startup enforces `waypoint_threshold_m > 0.0`.
  - Startup enforces unique DIS `(site_id, application_id, entity_id)` triplets across `entities`.
  - Startup enforces `force_id in 0..=3`; unsupported values fail startup.
  - Startup enforces `flightgear.override_timeout_secs >= 0.0` when FlightGear bridge is configured.
  - Startup enforces `oms.la-cal.position_hz > 0.0` and `oms.la-cal.prd_hz > 0.0` when LA-CAL config is present.
  - Startup starts the background OWP connection manager when `[oms.la-cal]` is configured; connection failures are handled asynchronously and do not fail startup after the manager starts.
  - Startup resolves `SystemID`, `SubsystemID`, and `MissionID` via UUIDv5 generation using the provided or default realm (`namespace_uuid`) and names (`system_name`, `subsystem_name`, `mission_name`) when explicit UUID overrides (`system_uuid`, `subsystem_uuid`) are not provided.
- **Error handling:**
  - Read/parse/validation errors are fatal startup errors.
- **Security:**
  - Local file trust model; no signature/authentication.
- **Source:** `src/config.rs`, `src/main.rs`, `tests/config_contract.rs`

### 3) JSBSim TCP console responses
- **Direction:** Input
- **External party:** JSBSim process/container
- **Transport:** TCP, line-oriented text protocol
- **Schema:**
  - Expected command responses for:
    - `set <prop> <value>` → line ending with `set successful`
    - `get <prop>` → `name = value`
    - `iterate <n>` → line ending with `Iterations performed`, followed by a state-sync polling loop waiting for `simulation/sim-time-sec` to reach the deterministic frame target.
- **Validation:**
  - Response format is parsed strictly, including exact property-name match for `get` responses before numeric parsing.
  - Runtime JSBSim socket read timeout is `2s`, bounding per-read stall exposure in the barrier-based tick loop.
  - Connection startup retries for availability and trim cycle.
- **Error handling:**
  - Connection/read/parse failures propagate as errors.
  - Startup is fail-fast for flying entities: if JSBSim init fails, process startup fails instead of skipping the entity.
  - Ctrl-C cancellation interrupts JSBSim startup retry waits.
  - In sim loop, step/read failures mark flying entity `Dead` (entity stops stepping and publication).
- **Security:**
  - No auth/TLS in protocol; intended for trusted network/container topology.
- **Reliability semantics:**
  - Best-effort request/response over TCP; no protocol-level retries beyond connection/startup logic.
  - `2s` read-timeout policy limits sibling entity stall impact from one slow JSBSim connection, at the cost of earlier entity death under prolonged slowness.
- **Source:** `src/fdm.rs`, `src/sim.rs`

### 4) FlightGear controls ingress (`FGNetCtrls`)
- **Direction:** Input
- **External party:** FlightGear or compatible controls sender
- **Transport:** UDP datagram to configured receive socket
- **Schema:**
  - Binary `FGNetCtrls` packet, big-endian
  - Required protocol version: `27`
  - Required packet size: exactly `744` bytes
- **Validation:**
  - Size mismatch or unsupported version is rejected as malformed.
- **Error handling:**
  - Malformed packet: drop + warn log + continue (`Ok(None)` at bridge API).
  - No packet available: non-blocking `Ok(None)`.
  - Real socket I/O error: `Err` from bridge receive call.
  - In sim loop manual override mode, missing packets are tolerated until `flightgear.override_timeout_secs` elapses, then manual override disengages and waypoint autopilot resumes.
- **Security:**
  - No authentication/integrity on UDP payloads.
- **Reliability semantics:**
  - UDP at-most-once; ordering and delivery not guaranteed.
- **Source:** `src/flightgear.rs`, `tests/flightgear_contract.rs`, `tests/sim_unit.rs`

### 5) Prometheus Metrics HTTP Endpoint
- **Direction:** Input (Pull)
- **External party:** Prometheus scraper
- **Transport:** HTTP GET `/metrics` over TCP to `admin_bind_addr`
- **Schema:**
  - Prometheus text exposition format
  - Exports `supercell_ticks_total`, `supercell_entities_active`, `supercell_dis_pdus_published_total`, `supercell_dis_publish_errors_total`, `supercell_owp_updates_total`, `supercell_waypoints_reached_total`, `supercell_fdm_errors_total`, `supercell_tick_duration_seconds`
- **Validation:**
  - Standard Admin server `/metrics` HTTP routing.
- **Error handling:**
  - Exporter startup logs a warning if initialization fails.
- **Security:**
  - No authentication or TLS. Intended for internal cluster scraping.
- **Reliability semantics:**
  - Provides point-in-time counter and gauge values.
- **Source:** `src/telemetry.rs`

## Outputs

### 6) DIS EntityState PDU publication
- **Direction:** Output
- **External party:** DIS listeners/consumers on network
- **Transport:** UDP send to configured `dis.multicast_addr:dis.port`
- **Schema:**
  - DIS v7 `EntityState` PDU (`pduType=1`)
  - Header exercise ID is `dis.exercise_id` (already 8-bit validated at parse time)
  - Header timestamp is populated as an absolute timestamp based on the system clock
  - Position: WGS-84 geodetic converted to ECEF
  - Velocity: NED converted to ECEF
  - Orientation: NED Euler converted to DIS ECEF Euler
  - Dead reckoning:
    - Fixed entities: `StaticNonmovingEntity`
    - Flying entities: `DRM_RVW...` with ECEF linear acceleration + body-axis angular velocity
  - Marking: non-ASCII removed, truncated to 11 ASCII chars
  - Force ID mapping supports 0..3 directly; defensive fallback to `Other` for out-of-range direct builder callers
- **Validation:**
  - Socket target and bind/interface settings validated during publisher construction.
- **Error handling:**
  - Publisher construction errors are fatal at startup.
  - Per-tick publish errors are logged; simulation loop continues.
- **Security:**
  - No auth/encryption.
- **Reliability semantics:**
  - UDP at-most-once.
- **Source:** `src/dis.rs`, `src/main.rs`, `src/sim.rs`, `docs/dis-entity-state-pdu.md`, `docs/coordinate-pipeline.md`

### 7) FlightGear FDM egress (`FGNetFDM`)
- **Direction:** Output
- **External party:** FlightGear or compatible cockpit consumer
- **Transport:** UDP datagram to configured `flightgear.fdm_send_addr:flightgear.fdm_send_port`
- **Schema:**
  - Binary `FGNetFDM` packet, big-endian
  - Protocol version `24`
  - Encoded size exactly `408` bytes
  - Units/frame mapping:
    - lat/lon in radians
    - orientation in local NED radians
    - velocities in ft/s
    - altitude field sourced from `EntityState.altitude_msl_m`
- **Validation:**
  - Destination address parsed during bridge setup.
- **Error handling:**
  - FDM egress is owned by the simulation interpolation thread; `FlightGearBridge` no longer exposes a bridge-level FDM send API.
  - Interpolation thread sends best-effort at ~60 Hz; individual send errors are ignored.
  - If shared interpolation state lock is poisoned, the interpolation thread logs and exits cleanly.
- **Security:**
  - No auth/encryption.
- **Reliability semantics:**
  - UDP at-most-once.
- **Source:** `src/flightgear.rs`, `src/sim.rs`

### 8) JSBSim TCP console commands
- **Direction:** Output
- **External party:** JSBSim process/container
- **Transport:** TCP, line-oriented text protocol
- **Schema:**
  - `set`, `get`, and `iterate` commands as plain text lines.
  - Runtime writes AP/FCS properties each tick (e.g., `ap/*`, `fcs/throttle-cmd-norm`).
- **Validation:**
  - Command responses are validated (success suffix / parseable `get` values).
- **Error handling:**
  - Per-call errors propagate via `Result`.
  - In sim loop, `step`/`read_state` failures mark entity dead.
  - In sim loop, AP/FCS control-write failures are logged and mark only the affected entity dead.
- **Security:**
  - No auth/TLS in protocol.
- **Source:** `src/fdm.rs`, `src/sim.rs`

### 9) OMS LA-CAL OWP WebSocket connection
- **Direction:** Output
- **External party:** Sleet LA-CAL OWP endpoint
- **Transport:** Outbound plain WebSocket (`ws://`) over TCP to `oms.la-cal.ws_url`
- **Schema:**
  - Startup performs a standard WebSocket client handshake when `[oms.la-cal]` is present.
  - The runtime emits `mission.position-report`, `mission.system-status`, `mission.navigation-report`, and `mission.route-plan` OWP `PUB` frames containing JSON-serialized UCI payloads. `mission.route-plan` is only published for entities that have an active mission waypoint sequence.
  - Emission rates are governed by `oms.la-cal.position_hz` and `oms.la-cal.prd_hz`.
  - All outbound UCI messages have `MessageHeader.Mode` explicitly set to `SIMULATION` (SIMULATED mode).
  - All outbound UCI messages have `MessageHeader.Timestamp` set to the current system wall-clock time (`xs:dateTime` string in UTC), ensuring downstream staleness/latency checks avoid internal simulation clock drift.
  - The `mission.position-report` message populates `SimulationTargetNumber` using a 64-bit integer representation of the DIS triplet (`(site_id << 32) | (application_id << 16) | entity_id`) to provide an explicit truth-mapping to the underlying simulated platform.
  - All outbound UCI messages include a `MissionID` in the `MessageHeader`, generated via UUIDv5 from the resolved `MISSION_NAME` and `NAMESPACE_UUID`.
  - All outbound UCI messages include explicitly configured `SecurityInformation` fields (`classification` and `owner_producer`), defaulting to `U` and `USA`.
- **Validation:**
  - `oms.la-cal.ws_url` must be accepted by the WebSocket client as a valid URL.
  - The current client build supports plain `ws://`; `wss://` connection attempts fail because TLS support is not enabled.
- **Error handling:**
  - Connection and handshake failures are logged and retried with linear backoff.
  - Remote close frames, dropped TCP connections, and WebSocket read errors trigger reconnect.
  - OWP network failures do not terminate startup or the simulation loop after the background manager starts.
- **Security:**
  - No TLS, authentication, or application payload signing is currently configured by SuperCell.
- **Reliability semantics:**
  - Connection maintenance is best-effort.
  - Reconnect backoff starts at `1s`, increases linearly, and caps at `5s`.
- **Source:** `src/owp.rs`, `src/main.rs`, `tests/owp_connection.rs`

### 10) Operational logs
- **Direction:** Output
- **External party:** Operator log sink (stdout/stderr collector)
- **Transport:** Structured text logs via `tracing`
- **Schema:**
  - Event-style logs with fields (entity IDs, tick, errors, state values).
- **Validation:**
  - Log level controlled by `RUST_LOG`/env filter.
- **Error handling:**
  - Logging failures are not surfaced as runtime errors.
- **Security:**
  - No redaction guarantees; avoid placing secrets in config/inputs.
- **Source:** `src/main.rs`, `src/sim.rs`, `src/fdm.rs`, `src/dis.rs`, `src/flightgear.rs`, `src/owp.rs`

### 11) Admin HTTP endpoints
- **Direction:** Output
- **External party:** Container runtime/orchestrator health probe or HTTP client
- **Transport:** HTTP GET over TCP to configured `admin_bind_addr`
- **Schema:**
  - Path: `/health`
    - Content: HTTP 200 OK (`{"status":"ok"}`) on success. HTTP 503 (`STALE`) if the first simulation tick does not occur within 60 seconds of startup, or if the simulation loop becomes stale later.
    - Update cadence: Live check against the simulation tick timestamp.
  - Path: `/ready`
    - Content: HTTP 200 OK when startup is complete and ready to step, HTTP 503 when still initializing.
  - Path: `/status`
    - Content: HTTP 200 OK returning JSON `{"status":"starting|ready","ready":true|false,"version":"..."}`.
- **Validation:**
  - Docker runtime `HEALTHCHECK` uses a local CLI invocation to check HTTP health status via `supercell --health-check [PORT]`.
- **Error handling:**
  - Failed probes result in `unhealthy` container state.
- **Security:**
  - Unauthenticated local bind.
- **Source:** `src/admin.rs`, `Containerfile`

### 12) OWP UCI NavigationReportMt publication
- **Direction:** Output
- **External party:** OMS LA-CAL Router / Sleet OWP WebSocket endpoint
- **Transport:** WebSocket publish frame over OWP connection to topic `mission.navigation-report`
- **Schema:**
  - `NavigationReportMt` UCI v2.5 JSON structure
  - Encodes navigation subsystem status (`Normal` contingency, `Actual` source)
  - Indicates the active navigation capability: `MISSION_PLAN_NAVIGATION` or `MANUAL_NAVIGATION` depending on manual-override and waypoint configuration.
  - Does NOT encode actual route or waypoint coordinate data.
- **Validation:**
  - Published at the same rate as `mission.system-status` (`prd_hz`).
- **Error handling:**
  - OWP connection manager intercepts errors, logs them, and eventually triggers reconnect backoff if the underlying WebSocket fails.
- **Reliability semantics:**
  - At-most-once per tick publish interval.
- **Source:** `src/owp.rs`

### 13) OTLP Trace Export
- **Direction:** Output
- **External party:** OpenTelemetry Collector / Jaeger / Observability Backend
- **Transport:** gRPC over TCP to configured `otlp_endpoint`
- **Schema:**
  - OTLP gRPC trace payloads exported via `tonic`
- **Validation:**
  - Traces are exported asynchronously via Tokio batch exporter.
- **Error handling:**
  - Export failures are logged internally by the OpenTelemetry SDK.
- **Security:**
  - Standard OTLP over gRPC. Endpoint determines if TLS is used (e.g. `https://`).
- **Reliability semantics:**
  - Batching and background export; drops spans if exporter queue fills up.
- **Source:** `src/telemetry.rs`

## Notes

- JSBSim-backed semantic integration tests live in `tests/pipeline.rs` and are currently `#[ignore]` for local/manual execution; CI wiring for these ignored tests is future work.
- `docs/dis-entity-state-pdu.md` and `docs/coordinate-pipeline.md` provide deeper field-level protocol details; this file is the contract index and policy surface.

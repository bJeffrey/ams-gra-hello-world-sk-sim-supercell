# Accelerated Scenario Time Implementation Plan

## Purpose

Add deterministic, configurable scenario-time support so Open Arsenal services can run faster than real time while continuing to communicate through OMS Language-Agnostic CAL (LA-CAL).

The associated cross-service DIS publication, `PositionReportDetailed`, and
`RoutePlan` work is tracked in
[SuperCell FTRT DIS/UCI Integration Plan](ftrt_dis_uci_integration_plan.md).

The implementation must avoid changing the host operating-system clock. Instead, services will use an injected scenario clock for simulation state, UCI timestamps, event timing, and publication cadence, while retaining a real monotonic clock for transport and operational concerns.

This file is intended to be placed at the top level of a local working repository and used as the implementation guide for Codex.

---

## Primary Goals

1. Run the simulation at configurable rates such as `1x`, `2x`, `10x`, or unpaced.
2. Preserve a fixed simulation integration step independent of wall-clock pacing.
3. Stamp all UCI messages with authoritative scenario time.
4. Schedule CAL publications according to scenario time.
5. Preserve real monotonic timing for:
   - socket timeouts;
   - reconnect backoff;
   - watchdogs;
   - health checks;
   - performance metrics;
   - logging intervals.
6. Keep simulation execution deterministic where practical.
7. Maintain backward-compatible real-time behavior by default.
8. Provide realtime, scaled, unpaced, and stepped operation as native
   SuperCell capabilities with no runtime or library dependency on
   `ai-bm-sim`.

`ai-bm-sim` may optionally replace the local tick source for coordinated
multi-service runs. That adapter adds ecosystem epochs, barriers, replay, and
durable control delivery; it must reuse, rather than replace, SuperCell's
clock, pacing, and `step_once` foundations.

---

## Non-Goals

- Do not change the host or container system clock.
- Do not depend on `libfaketime` for the production design.
- Do not make CAL transport timeouts advance at the accelerated scenario rate.
- Do not rewrite the OMS or UCI schemas.
- Do not require every service to run in the same process.
- Do not couple scenario-time advancement to network latency.
- Do not require `ai-bm-sim`, CAL, DIS, NATS, or JetStream to exercise the
  local clock, scaled/unpaced pacing, or step API in unit and component tests.

---

## Repositories and Files Initially in Scope

### SuperCell

Repository:

```text
open-arsenal/ams-gra-hello-world-sk-sim-supercell
```

Primary files:

```text
src/sim.rs
src/owp.rs
src/config.rs
src/main.rs
docs/configuration.md
config/default.toml
```

Likely new files:

```text
src/time.rs
tests/scenario_time.rs
docs/scenario-time.md
```

### IR Search and Track

Repository:

```text
open-arsenal/ams-gra-hello-world-sk-skills-ir-search-and-track
```

Primary files:

```text
src/Application.cpp
src/Application.hpp
src/C2Client.cpp
src/C2Client.hpp
src/Config.cpp
src/Config.hpp
src/main.cpp
docs/configuration.md
```

Likely new files:

```text
src/ScenarioClock.hpp
src/ScenarioClock.cpp
tests/ScenarioClockTests.cpp
```

### Additional Services

After SuperCell and IR Search and Track are complete, search all participating repositories for direct uses of:

```text
SystemTime::now
OffsetDateTime::now_utc
chrono::system_clock::now
time(nullptr)
clock_gettime(CLOCK_REALTIME, ...)
Utc::now
Instant::now
steady_clock::now
tokio::time::interval
std::thread::sleep
sleep_for
sleep_until
```

Classify every occurrence as either:

```text
SCENARIO TIME
REAL MONOTONIC TIME
```

Do not replace real monotonic uses blindly.

---

## Existing Behavior to Change

### SuperCell simulation pacing

The simulation currently derives both values from `tick_hz`:

```text
simulation step = 1 / tick_hz
wall-clock period = 1 / tick_hz
```

It then:

1. steps JSBSim by the simulation step;
2. publishes state;
3. sleeps for the remainder of the same wall-clock period.

This couples simulation time to real time.

### SuperCell CAL timestamps

The OWP publisher currently generates UCI timestamps from wall-clock UTC using:

```rust
time::OffsetDateTime::now_utc()
```

Those timestamps are reused in several UCI fields.

### SuperCell CAL publication cadence

The OWP publisher currently uses real-time Tokio intervals for:

```text
PositionReport
SystemStatus
RoutePlan
NavigationReport
```

### IR Search and Track timestamps

The service currently creates outgoing UCI timestamps using:

```cpp
std::chrono::system_clock::now()
```

This can cause source data and generated observations to use different clock domains.

---

# Target Architecture

## Clock domains

Every service must distinguish between two clock domains.

### Scenario clock

Use for:

- simulation propagation;
- sensor collection time;
- measurement time;
- track time;
- UCI message timestamps;
- event scheduling;
- simulated task deadlines;
- simulated time-to-live;
- simulated publication cadence;
- replay and stepped execution.

### Real monotonic clock

Use for:

- network connection timeout;
- WebSocket timeout;
- reconnect backoff;
- thread coordination;
- health monitoring;
- process watchdogs;
- wall-performance measurements;
- wall-rate limiting;
- logging throttles;
- operator UI responsiveness.

---

## Supported clock modes

Implement at least:

```text
realtime
scaled
unpaced
stepped
```

### Realtime

```text
scenario_time_rate = 1.0
```

Scenario time advances at the same rate as wall time.

### Scaled

```text
scenario_time_rate > 0.0
```

Example:

```text
rate = 10.0
```

Ten simulated seconds pass per real second.

### Unpaced

The simulation advances as quickly as computation permits.

No sleeping is performed to preserve a wall-clock rate.

### Stepped

Scenario time advances only when explicitly commanded.

This mode is useful for deterministic testing and future orchestration.

Stepped mode can initially be implemented internally without a remote control API. A later phase may add an admin endpoint or CAL command.

---

## Scenario-time equation

For real-time and scaled modes:

```text
scenario_now =
    scenario_epoch
    + rate * (monotonic_now - monotonic_anchor)
```

For simulation loops that own authoritative advancement, prefer discrete advancement:

```text
scenario_time += simulation_dt
```

The simulation loop should be authoritative for state time. The affine clock equation can still be useful for services that do not receive timestamped source events.

---

# Configuration Design

Add a top-level SuperCell time configuration.

```toml
[time]
mode = "realtime"
rate = 1.0
epoch = "2026-01-01T00:00:00Z"
simulation_hz = 100.0
```

Recommended definitions:

| Field | Type | Default | Meaning |
|---|---:|---:|---|
| `mode` | string | `"realtime"` | `realtime`, `scaled`, `unpaced`, or `stepped` |
| `rate` | float | `1.0` | Scenario seconds per wall second; used by `scaled` |
| `epoch` | RFC 3339 string | current UTC at startup | Scenario timestamp at simulation start |
| `simulation_hz` | float | existing `tick_hz` | Simulation integration frequency |
| `max_wall_publish_hz` | float, optional | unset | Optional transport-protection limiter |

Backward compatibility:

- Continue accepting existing `tick_hz`.
- During migration, interpret `tick_hz` as `time.simulation_hz` when the new field is absent.
- In SuperCell, change raw config storage so `tick_hz` and `[time]` can coexist under
  `#[serde(deny_unknown_fields)]`. Prefer `tick_hz: Option<f64>`,
  `time: Option<TimeConfig>`, and resolved helper methods such as
  `simulation_hz()` and `time_settings()`.
- If both `tick_hz` and `time.simulation_hz` are provided, prefer
  `time.simulation_hz` and log a deprecation warning for `tick_hz`.
- Preserve current one-to-one behavior when no `[time]` table is present.

Potential final configuration:

```toml
[time]
mode = "scaled"
rate = 10.0
epoch = "2026-01-01T00:00:00Z"
simulation_hz = 100.0

[oms.la-cal]
position_hz_sim = 10.0
prd_hz_sim = 1.0
max_publish_hz_wall = 500.0
```

During initial implementation, retain existing configuration keys:

```text
position_hz
prd_hz
```

but document that they mean reports per simulated second.

---

# Core Data Types

## Rust clock interface

Create:

```text
src/time.rs
```

Suggested interface:

```rust
use std::time::{Duration, Instant};
use time::OffsetDateTime;

#[derive(Debug, Clone)]
pub enum TimeMode {
    Realtime,
    Scaled { rate: f64 },
    Unpaced,
    Stepped,
}

#[derive(Debug, Clone)]
pub struct ScenarioClock {
    mode: TimeMode,
    epoch: OffsetDateTime,
    scenario_elapsed: Duration,
    monotonic_anchor: Instant,
}

impl ScenarioClock {
    pub fn new(mode: TimeMode, epoch: OffsetDateTime) -> Self;

    pub fn now(&self) -> OffsetDateTime;

    pub fn elapsed(&self) -> Duration;

    pub fn advance(&mut self, dt: Duration);

    pub fn reset(&mut self, epoch: OffsetDateTime);

    pub fn wall_period_for(&self, simulation_dt: Duration) -> Option<Duration>;

    pub fn is_unpaced(&self) -> bool;

    pub fn is_stepped(&self) -> bool;
}
```

Important design rule:

```text
Simulation state time is advanced explicitly by the simulation loop.
```

Do not derive authoritative state time solely from `Instant::elapsed()` because that can introduce wall-scheduling jitter and reduce determinism.

## Timestamped simulation state

Replace bare state updates passed to DIS and OWP with a shared timestamped
state object:

```rust
#[derive(Clone, Debug)]
pub struct TimedEntityState {
    pub state: EntityState,
    pub scenario_time: OffsetDateTime,
    pub tick: u64,
}
```

Place this type in a shared module such as `entity.rs` or `time.rs`, not inside
the OWP implementation. The simulation loop owns the authoritative timestamp;
downstream publishers must not generate a fresh payload/event timestamp.

The OWP publisher must use `scenario_time` from this object. The DIS publisher
should also use it for `EntityStatePdu` timestamps unless the project explicitly
documents DIS as wall-time stamped for interoperability.

## Optional clock abstraction for non-simulation Rust services

For services without authoritative state updates:

```rust
pub trait Clock: Send + Sync {
    fn now(&self) -> OffsetDateTime;
}
```

Implement:

```text
SystemClock
ScaledClock
ManualClock
```

## C++ clock interface

Suggested interface:

```cpp
class ScenarioClock {
public:
  virtual ~ScenarioClock() = default;
  virtual std::chrono::system_clock::time_point now() const = 0;
};

class SystemScenarioClock final : public ScenarioClock {
public:
  std::chrono::system_clock::time_point now() const override;
};

class ScaledScenarioClock final : public ScenarioClock {
public:
  ScaledScenarioClock(
      std::chrono::system_clock::time_point epoch,
      std::chrono::steady_clock::time_point anchor,
      double rate);

  std::chrono::system_clock::time_point now() const override;
};
```

For IR Search and Track, source-event timestamps should take precedence over local clock timestamps.

---

# Implementation Phases

## Phase 1 — Add the SuperCell scenario clock

### Tasks

1. Add `src/time.rs`.
2. Define:
   - `TimeMode`;
   - `ScenarioClock`;
   - configuration parsing;
   - validation.
3. Add clock unit tests.
4. Export the new module from the crate root.
5. Add the shared `TimedEntityState` type.
6. Add resolved configuration helpers so runtime code reads
   `simulation_hz` and time settings through one compatibility layer.
7. Preserve current behavior by default.

### Validation

Reject:

```text
rate <= 0
simulation_hz <= 0
invalid RFC 3339 epoch
scaled mode without a valid rate
```

For unpaced and stepped modes, `rate` may be ignored but must not produce ambiguous behavior.

### Acceptance criteria

- Default configuration behaves like the existing implementation.
- `ScenarioClock::advance(100 ms)` advances scenario time by exactly 100 ms.
- Time formatting produces valid UCI-compatible RFC 3339 UTC strings.
- Tests do not depend on sleeping.
- Existing configs with only `tick_hz` still parse and resolve to realtime
  `simulation_hz = tick_hz`.

---

## Phase 2 — Separate simulation integration from wall pacing

### Target

```text
src/sim.rs
```

### Current conceptual behavior

```rust
let tick_duration = Duration::from_secs_f64(1.0 / tick_hz);
let dt_sec = 1.0 / tick_hz;
...
handle.step(dt_sec)?;
...
sleep(tick_duration - elapsed);
```

### New conceptual behavior

```rust
let simulation_dt = Duration::from_secs_f64(1.0 / simulation_hz);

while running {
    let wall_start = Instant::now();

    scenario_clock.advance(simulation_dt);
    let timed_states = simulation.step_once(simulation_dt, scenario_clock.now(), tick);
    publish_timestamped_states(timed_states);

    match scenario_clock.wall_period_for(simulation_dt) {
        Some(wall_period) => sleep_remaining(wall_start, wall_period),
        None => {}
    }
}
```

### Mode-specific pacing

#### Realtime

```text
wall_period = simulation_dt
```

#### Scaled

```text
wall_period = simulation_dt / rate
```

#### Unpaced

```text
no sleep
```

#### Stepped

Initial implementation options:

```text
A. block on an internal step channel;
B. expose Simulation::step_once();
C. run only from tests.
```

Implement option B initially. `Simulation::step_once()` should contain the
entity stepping, derived-state updates, DIS publication, OWP state update, and
per-tick metrics that are currently embedded in `Simulation::run()`. The
`run()` method should become the pacing wrapper around the shared step path.

### Important details

- JSBSim must continue receiving the simulation step, not wall elapsed time.
- Acceleration calculations must use `simulation_dt`.
- Waypoint timing must follow state updates, not wall time.
- Decide and document whether `settle_secs` is scenario time or wall time.
  For deterministic accelerated runs, prefer scenario time; if wall settle is
  required for JSBSim or operator interaction, rename or document it explicitly.
- Manual-control staleness may remain wall monotonic initially because it is
  tied to an external interactive FlightGear input stream.
- FlightGear smoothing is a visualization concern and may remain wall monotonic.
- `FGNetFDM.cur_time` should be classified explicitly. Prefer wall time for
  cockpit visualization unless a FlightGear consumer requires scenario time.
- Heartbeat age for process health should remain wall monotonic.
- Add scenario time to metrics separately from wall performance.

### Acceptance criteria

At `simulation_hz = 100`:

- Realtime mode advances approximately 1 simulated second per real second.
- Scaled `rate = 10` advances approximately 10 simulated seconds per real second.
- Unpaced mode completes a fixed number of ticks faster than realtime when CPU permits.
- The state after N ticks is equivalent across realtime, scaled, and unpaced modes, subject to existing nondeterministic external inputs.
- Unit and integration tests can drive exactly N ticks without sleeping.

---

## Phase 3 — Pass authoritative time into DIS and OWP publishers

### Targets

```text
src/sim.rs
src/dis.rs
src/owp.rs
```

### Tasks

1. Change simulation publication handoff from bare `EntityState` to
   `TimedEntityState`.

2. Change the OWP watch channel from:

```rust
watch::Sender<Option<EntityState>>
```

to:

```rust
watch::Sender<Option<TimedEntityState>>
```

3. Change:

```rust
update_entity_state(state)
```

to:

```rust
update_entity_state(timed_state)
```

4. Remove OWP payload timestamp generation from:

```rust
OffsetDateTime::now_utc()
```

5. Format OWP timestamps from:

```rust
timed_state.scenario_time
```

6. Use the same authoritative timestamp consistently in:
   - `MessageHeader.Timestamp`;
   - `Point4D.Timestamp`;
   - `Velocity3D.Timestamp`;
   - message-data timestamps;
   - route and navigation reports where applicable.

7. Add a DIS timestamp conversion path from `TimedEntityState.scenario_time`
   and use it when building `EntityStatePdu` headers.

8. Keep reconnect and socket behavior on Tokio real time.

### Acceptance criteria

- No UCI payload timestamp in `owp.rs` is generated from wall UTC.
- DIS `EntityStatePdu` timestamps come from scenario time, or an explicit
  documented interoperability exception explains why DIS remains wall stamped.
- All reports produced from one state update use the same scenario timestamp.
- Existing OWP connection tests continue to pass.
- A new test verifies that a known scenario timestamp appears in every expected UCI field.

---

## Phase 4 — Schedule CAL publications using scenario time

### Current behavior

```rust
tokio::time::interval(...)
```

This schedules reports per real second.

### Required behavior

Schedule reports per simulated second.

Suggested state:

```rust
struct PublicationSchedule {
    next_position_time: OffsetDateTime,
    next_prd_time: OffsetDateTime,
    position_period: Duration,
    prd_period: Duration,
}
```

On each received state update:

```rust
while state.scenario_time >= schedule.next_position_time {
    publish_position(...);
    schedule.next_position_time += position_period;
}

while state.scenario_time >= schedule.next_prd_time {
    publish_periodic_reports(...);
    schedule.next_prd_time += prd_period;
}
```

### Catch-up policy

Use one configurable or documented policy.

Recommended initial policy:

```text
Publish at most one report of each type per received state update.
Advance the next deadline past the current scenario time.
```

This avoids bursts after delays.

Alternative future policy:

```text
Publish every missed report.
```

That mode is more faithful but can overload CAL.

### Suggested implementation

Create a small pure scheduler type first, then wire it into the OWP connection
loop:

```rust
struct PublicationDue {
    position: bool,
    periodic: bool,
    coalesced_position: u64,
    coalesced_periodic: u64,
}
```

The scheduler should have no WebSocket, Tokio, or wall-clock dependency. Unit
tests should cover initial deadlines, exact-deadline publication, coalescing,
and independent position/periodic rates.

```rust
fn advance_deadline_past(
    deadline: &mut OffsetDateTime,
    period: Duration,
    current: OffsetDateTime,
) {
    while *deadline <= current {
        *deadline += period;
    }
}
```

### Wall-rate limiter

Optionally protect CAL with a wall-monotonic limiter:

```text
max_publish_hz_wall
```

This limiter should:

- delay or coalesce output;
- not alter scenario timestamps;
- not change simulation advancement.

### Acceptance criteria

With:

```text
simulation_hz = 100
position_hz = 10
rate = 10
```

the service produces approximately:

```text
10 reports per simulated second
100 reports per wall second
```

subject to the optional wall-rate limiter.

---

## Phase 5 — Update UCI mode and timestamp semantics

### Tasks

1. Ensure simulation-produced messages use:

```text
Mode = SIMULATION
```

2. Search for outgoing messages using:

```text
Mode = LIVE
```

3. Make mode configurable only where a service legitimately supports both live and simulated operation.
4. Document timestamp semantics:
   - time of state validity;
   - time of sensor collection;
   - time of report creation;
   - time of receipt.

### Acceptance criteria

- Simulated SuperCell reports use simulation mode.
- Simulated IR Search and Track reports use simulation mode.
- Source collection time is not silently replaced with local report-generation time.

---

## Phase 6 — Modify IR Search and Track

### Targets

```text
src/Application.cpp
src/Application.hpp
src/Config.cpp
src/Config.hpp
src/main.cpp
src/C2Client.cpp
src/C2Client.hpp
```

### Tasks

1. Replace direct calls to:

```cpp
current_uci_timestamp()
```

with an injected clock or source timestamp.
2. Prefer source-event time in this order:
   1. image/frame collection timestamp;
   2. associated PositionReport state timestamp;
   3. injected scenario clock;
   4. system UTC only in live mode.
3. Ensure the outgoing `ObservationMeasurementReport` uses internally consistent time.
4. Make message mode configurable:
   - `LIVE`;
   - `SIMULATION`.
5. Preserve `steady_clock` for:
   - operation timeouts;
   - logging intervals;
   - debug-frame throttling;
   - health and retry behavior.

### Recommended first implementation

If the image frame header has a valid collection timestamp:

```text
use frame timestamp
```

Otherwise:

```text
use the PositionReport InertialState.Position.Timestamp
```

Do not create a fresh local system timestamp when the measurement is derived from simulated source data.

### Acceptance criteria

- No simulated observation timestamp comes from `system_clock::now()`.
- Position reference and observation measurement timestamps are in the same scenario clock domain.
- Transport timeouts still use `steady_clock`.
- Live mode retains current system-clock behavior.

---

## Phase 7 — Add scenario-clock propagation between services

For distributed services, use one of the following patterns.

### Preferred pattern: source-event timestamps

Every message carries authoritative event time.

Consumers derive output event times from input event times.

Advantages:

- no separate clock-sync protocol is required for most processing;
- deterministic replay is easier;
- services can process unpaced data;
- network delay does not alter event time.

### Optional pattern: scenario-time status topic

Add a scenario-time message only if services need a continuously advancing clock when no data arrives.

Potential topic:

```text
simulation.time-status
```

Potential contents:

```json
{
  "ScenarioTime": "2026-01-01T00:00:12.300Z",
  "Rate": 10.0,
  "Mode": "SCALED",
  "Paused": false,
  "Tick": 1230
}
```

Do not add this until a concrete consumer requires it.

### Future control topic

Potential topic:

```text
simulation.time-control
```

Potential commands:

```text
START
PAUSE
RESUME
STEP
SET_RATE
RESET
```

This is a later capability, not required for initial accelerated execution.

---

## Phase 8 — Audit all services

Search each repository for clock and timer usage.

Create an audit table:

| Repository | File | Call | Classification | Action |
|---|---|---|---|---|
| SuperCell | `src/sim.rs` | `Instant::now()` | wall monotonic | retain for pacing |
| SuperCell | `src/owp.rs` | `OffsetDateTime::now_utc()` | scenario payload | replace |
| SuperCell | `src/dis.rs` | `SystemTime::now()` | scenario DIS header | replace or document exception |
| SuperCell | `src/flightgear.rs` | `SystemTime::now()` | FlightGear visual packet | classify; likely retain wall time |
| IR S&T | `src/Application.cpp` | `system_clock::now()` | scenario payload | replace |
| IR S&T | `src/Application.cpp` | `steady_clock::now()` | logging throttle | retain |

Required classification rule:

```text
Never replace a timer until its semantic purpose is documented.
```

---

# CAL and Middleware Review

Accelerating payload time does not automatically accelerate CAL internal timing.

Review the configured CAL implementation for:

- time-based filtering;
- expiration or shelf-life;
- queue depth;
- reliable delivery;
- slow consumers;
- retained-message behavior;
- ordering policy;
- back pressure;
- maximum frame size;
- subscription-group behavior.

Document whether each CAL behavior is based on:

```text
payload scenario timestamp
CAL ingress wall time
CAL server monotonic time
```

Do not assume payload timestamps control middleware expiration.

---

# Determinism Requirements

Where possible:

1. Step simulation with a fixed `simulation_dt`.
2. Advance scenario time exactly once per completed simulation tick.
3. Avoid deriving integration time from wall elapsed time.
4. Seed random-number generators explicitly.
5. Record:
   - scenario epoch;
   - simulation step;
   - time mode;
   - scale rate;
   - random seeds;
   - configuration hash;
   - software commit IDs.
6. Do not allow logging or network delays to alter state propagation.
7. For replay, use message event timestamps rather than receipt timestamps.

---

# Testing Plan

## Unit tests

### ScenarioClock

Test:

```text
default realtime configuration
scaled wall period calculation
unpaced no-sleep behavior
stepped explicit advancement
invalid rates
invalid epochs
timestamp formatting
exact discrete advancement
```

### Publication scheduler

Test:

```text
first deadline
on-time publication
missed deadline coalescing
large scenario-time jump
position and PRD independent rates
zero and invalid rates rejected
```

### UCI timestamp construction

Given:

```text
2026-01-01T00:00:05.250Z
```

assert that all relevant timestamp fields contain exactly that scenario time.

## Integration tests

### Fixed-tick equivalence

Run N ticks in:

```text
realtime
scaled 10x
unpaced
```

Compare final entity state within a defined tolerance.

### Rate test

Run a scenario for a fixed wall duration.

Measure:

```text
scenario_elapsed / wall_elapsed
```

Expected values:

```text
1x mode: approximately 1
10x mode: approximately 10
```

Use generous tolerance in CI because shared runners are noisy.

### CAL timestamp test

Capture outgoing OWP messages and verify:

- timestamps increase monotonically;
- timestamp increments match simulation advancement;
- no timestamp follows wall UTC during scaled execution;
- message mode is `SIMULATION`.

### Publication density test

For:

```text
10 simulated seconds
position_hz = 10
prd_hz = 1
```

expect approximately:

```text
100 PositionReport messages
10 periodic-report cycles
```

The exact expected count must match the chosen initial-publication and catch-up policy.

### Back-pressure test

Run at:

```text
10x
50x
unpaced
```

Observe:

- CAL queue growth;
- dropped messages;
- reconnects;
- subscriber lag;
- CPU load;
- memory growth.

---

# Metrics and Logging

Add metrics:

```text
supercell_scenario_time_seconds
supercell_scenario_rate_configured
supercell_scenario_rate_achieved
supercell_simulation_dt_seconds
supercell_wall_tick_duration_seconds
supercell_time_mode
supercell_owp_publish_lag_sim_seconds
supercell_owp_publish_rate_wall_hz
supercell_owp_publications_coalesced_total
```

Log at startup:

```text
time_mode
scenario_epoch
simulation_hz
simulation_dt
configured_rate
expected_wall_period
position_hz_sim
prd_hz_sim
```

Do not log every tick at normal log levels.

---

# Documentation Updates

Update:

```text
README.md
docs/configuration.md
docs/architecture.md
docs/contracts.md
```

Add:

```text
docs/scenario-time.md
```

The scenario-time document should define:

- clock domains;
- timestamp ownership;
- rate semantics;
- publication-rate semantics;
- replay behavior;
- wall timeout behavior;
- limitations;
- CAL middleware caveats.

---

# Suggested Commit Sequence

## Commit 1

```text
Add scenario clock types, timed state, and configuration
```

Includes:

```text
src/time.rs
TimedEntityState
config parsing
resolved config helpers
unit tests
documentation skeleton
```

## Commit 2

```text
Decouple simulation step from wall pacing
```

Includes:

```text
src/sim.rs
realtime/scaled/unpaced behavior
rate metrics
integration tests
```

## Commit 3

```text
Propagate authoritative scenario time with entity state
```

Includes:

```text
TimedEntityState
DIS timestamp source
OWP watch channel changes
timestamp formatting tests
```

## Commit 4

```text
Schedule OWP publications in scenario time
```

Includes:

```text
publication scheduler
coalescing policy
wall-rate limiter if needed
```

## Commit 5

```text
Update simulated UCI mode and timestamp consistency
```

Includes:

```text
Mode = SIMULATION
field-level timestamp checks
```

## Commit 6

```text
Inject scenario time into IR search and track
```

Includes:

```text
C++ clock interface
source timestamp precedence
configuration
tests
```

## Commit 7

```text
Document accelerated-time operation and CAL limits
```

Includes:

```text
operator documentation
example configurations
performance guidance
```

---

# Codex Working Instructions

When implementing this plan:

1. Inspect the current file before editing it.
2. Preserve existing public interfaces unless the phase explicitly changes them.
3. Prefer small, reviewable commits.
4. Add tests in the same change as each behavior.
5. Do not replace `Instant`, `steady_clock`, or Tokio timers until their semantic purpose is classified.
6. Do not use host-clock modification.
7. Do not use wall elapsed time as JSBSim integration time.
8. Keep default behavior backward compatible.
9. Run formatting, linting, and tests after each phase.
10. Update this plan with completed checkboxes and discovered constraints.

---

# Implementation Checklist

## SuperCell clock foundation

- [x] Add `TimeMode`.
- [x] Add `ScenarioClock`.
- [x] Add shared `TimedEntityState`.
- [x] Add `[time]` configuration.
- [x] Add resolved config helpers for `tick_hz` and `[time]` compatibility.
- [x] Preserve existing `tick_hz` compatibility.
- [x] Add validation.
- [x] Add clock unit tests.

## Simulation pacing

- [x] Separate `simulation_dt` from wall period.
- [x] Add realtime mode.
- [x] Add scaled mode.
- [x] Add unpaced mode.
- [x] Add `Simulation::step_once()` and bounded `step_ticks()` APIs for
  standalone tests and embedding. Add an operator/control-plane command source
  before treating `mode = "stepped"` as a continuously running executable.
- [x] Advance scenario time once per tick.
- [x] Treat `settle_secs` as scenario time; wall-monotonic FlightGear input
  staleness and interpolation remain operational/visual concerns.
- [x] Add scenario-elapsed and wall tick-duration metrics. Add a measured
  achieved-rate gauge after the sampling window is defined.
- [x] Add bounded fixed-tick equivalence coverage proving realtime, scaled,
  unpaced, and stepped modes produce identical FDM step counts, scenario
  elapsed time, and control writes after the same number of ticks.

## DIS and OWP/CAL

- [x] Pass scenario time to DIS.
- [x] Replace runtime DIS wall timestamping; retain the legacy wall-time
  convenience publisher for compatibility.
- [x] Pass scenario time and tick to OWP.
- [x] Remove payload use of `now_utc()` from the timed OWP publication path.
- [x] Build and test a pure publication scheduler.
- [x] Schedule reports in scenario time from incoming `TimedEntityState`.
- [x] Coalesce missed deadlines to at most one publication of each report
  family per received state update; record skipped deadline counts for debug
  diagnostics.
- [x] Preserve same-tick multi-platform updates through a bounded FIFO OWP
  event queue with independent per-platform schedules and wall-rate limiters.
- [x] Add optional wall-monotonic OWP publication-batch limiter configured by
  `time.max_wall_publish_hz`; excess due batches are coalesced without changing
  scenario timestamps or simulation advancement.
- [x] Verify generated PositionReportDetailed header, position, and velocity
  timestamps use the represented scenario state.
- [x] Ensure simulation message mode.
- [x] Add an opt-in live Sleet integration test that proves the generated
  PositionReportDetailed is accepted by UCI 2.5 schema validation and routed
  intact to a subscriber (`SUPERCELL_SLEET_E2E_URL`).
- [ ] Classify `FGNetFDM.cur_time` as wall visual time or scenario time.

## IR Search and Track

- [ ] Add C++ clock abstraction.
- [ ] Inject clock into `Application`.
- [ ] Prefer frame collection time.
- [ ] Fall back to PositionReport time.
- [ ] Use system UTC only in live mode.
- [ ] Preserve `steady_clock` timeouts.
- [ ] Make UCI mode configurable.
- [ ] Add timestamp consistency tests.

## Distributed behavior

- [ ] Audit other repositories.
- [ ] Classify every clock usage.
- [ ] Verify source-event time propagation.
- [ ] Determine whether a time-status topic is necessary.
- [ ] Document CAL wall-time behaviors.

## Validation

- [ ] Fixed-tick state equivalence test.
- [ ] 10x achieved-rate test.
- [ ] Unpaced test.
- [ ] CAL timestamp monotonicity test.
- [ ] Publication-density test.
- [ ] Back-pressure test.
- [ ] Documentation complete.

---

# Definition of Done

The work is complete when:

1. A scenario can run at `1x`, `10x`, and unpaced without changing the host clock.
2. The same fixed number of simulation ticks produces equivalent final state across pacing modes.
3. All simulated UCI payload timestamps derive from authoritative scenario time.
4. CAL publication rates are defined and enforced per simulated second.
5. Transport timeouts and reconnect behavior remain based on real monotonic time.
6. Simulated services use UCI simulation mode.
7. Downstream services do not mix scenario and wall timestamps in a single generated observation.
8. Automated tests cover clock advancement, pacing, timestamp propagation, and publication scheduling.
9. Documentation clearly states which time domain governs every major behavior.
10. Default configuration remains compatible with current real-time execution.

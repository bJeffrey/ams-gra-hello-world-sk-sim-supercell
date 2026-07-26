# SuperCell FTRT DIS/UCI Integration Plan

## Purpose

Define SuperCell's role as the platform-dynamics service in the FTRT
battle-management ecosystem. This plan complements
`docs/faster_than_real_time_development_plan.md` with the cross-service DIS and
UCI responsibilities required by Sensor Models, AST, and BMA.

## Boundary

```text
scenario tick + RoutePlan
          |
          v
   SuperCell / JSBSim
          |
          +--> DIS EntityStatePdu truth for blue and red
          +--> UCI PositionReportDetailed for cooperating platforms
          +--> tick completion and health
```

SuperCell owns achieved platform motion. A BMA route is a command/plan, not a
new platform position.

SuperCell also owns its basic simulation-time capability. Running at a fixed
scale, unpaced, or one explicit step at a time must not require `ai-bm-sim`.
`ai-bm-sim` integration is an optional control adapter for coordinated runs,
not the implementation of SuperCell's clock or pacing loop.

## Control Modes

- **Standalone:** SuperCell creates the authoritative local tick stream from
  configuration or its step API and supports `realtime`, `scaled`, `unpaced`,
  and `stepped` execution.
- **Ecosystem-controlled:** SuperCell disables local advancement and follows
  the authoritative tick stream supplied by `ai-bm-sim`, including timeline
  epochs, barriers, replay, and durable/JetStream-backed coordination where
  configured.
- Exactly one mode owns advancement for a run. Both modes use the same
  `step_once` dynamics path and scenario-time stamping logic.

## Required Inputs

- A local time configuration/step command, or authoritative
  `(scenario_time, dt, tick_id, timeline_epoch)` from the optional `ai-bm-sim`
  simulation control plane.
- Scenario/entity initialization and deterministic run identity.
- UCI `RoutePlan` updates for commanded cooperating platforms.
- Platform model, navigation limits, autopilot, and JSBSim configuration.

## Required Outputs

### DIS Truth

Publish `EntityStatePdu` for every configured simulated entity using:

- stable exercise/site/application/entity identity;
- scenario-derived DIS timestamp;
- WGS84/ECEF position and velocity;
- orientation, angular velocity, acceleration, appearance, and dead-reckoning
  fields supported by the platform model;
- explicit lifecycle/removal behavior.

DIS truth is consumed by Sensor Models and isolated training/evaluation tools.
It is not an operational BMA observation.

### PositionReportDetailed

Publish cooperating-platform navigation state through UCI/CAL. Populate
detailed kinematics and required covariance from the configured navigation
model. Do not invent a precision level merely to satisfy required schema
fields. Use stable platform identities shared with RoutePlan applicability.

### RoutePlan Consumption

- Apply plans to the addressed platform only.
- Support waypoint altitude, speed, sequence, and required arrival time.
- Define stable plan identity, version replacement, stale-plan rejection, and
  acknowledgement behavior.
- Feed accepted plans through navigation/autopilot dynamics.

## Timing Rules

- Advance JSBSim by fixed scenario `dt` regardless of wall pacing.
- Stamp DIS and UCI state from the tick being represented.
- Schedule UCI products in scenario time rather than Tokio wall intervals.
- Use monotonic wall time for sockets, reconnects, process health, and
  throughput measurements.
- In stepped/unpaced modes, acknowledge completion only after platform state
  and required outputs for the tick are committed to their output boundaries.

## Checklist

### Clock Integration

- [x] Add initial `TimeMode`, `ScenarioClock`, and time configuration types.
- [x] Add initial `TimedEntityState` domain type.
- [ ] Complete standalone scaled, unpaced, and stepped integration without
  linking to or running `ai-bm-sim`. Scaled and unpaced loop pacing is wired;
  explicit stepped execution remains.
- [x] Expose explicit local `step_once` and `step_ticks` APIs suitable for
  tests and embedding; repeated calls preserve local tick and scenario time.
- [ ] Add a selectable adapter that makes the simulation loop follow the
  ecosystem-authoritative tick.
- [ ] Add tick acknowledgement and timeline-epoch reset handling.

### DIS

- [x] Publish DIS `EntityStatePdu` from simulated entities.
- [x] Publish runtime DIS PDUs with deterministic scenario-time conversion;
  retain `current_dis_timestamp()` only for the backward-compatible publisher
  entry point.
- [ ] Add deterministic multi-entity golden-PDU tests.
- [ ] Add lifecycle, stale, and timeline-reset tests.
- [ ] Verify container multicast and unicast operation with Sensor Models.

### UCI/CAL

- [x] Publish initial UCI `PositionReport` and `RoutePlan` products.
- [x] Carry `TimedEntityState` into the OWP manager and derive PositionReport,
  SystemStatus, RoutePlan, and NavigationReport payload timestamps from the
  represented scenario state.
- [x] Replace wall-time publication intervals with a pure scenario-time
  scheduler using one-per-state coalescing for missed deadlines.
- [x] Add optional wall-monotonic OWP transport protection without coupling it
  to scenario advancement.
- [x] Publish `PositionReportDetailed` for every active cooperating/friendly
  flying platform, using per-platform scheduling and deterministic EGI source
  identities. Ownship retains its configured UCI IDs.
- [x] Populate required NED position/velocity covariance by propagating the
  configured one-sigma EGI timing uncertainty through velocity and
  acceleration.
- [x] Label the default scenario consistently with its available flight
  dynamics: all bundled `eagle1`/`bandit1`/`bandit2` aliases are C172P-derived,
  use C172 wire markings, and retain DIS category 84/subcategory 1. Distinct
  fighter models and flight-envelope acceptance remain future model additions.
- [ ] Consume externally produced `RoutePlan` products.
- [ ] Add plan version, applicability, arrival-time, and rejection tests.

### Acceptance

- [ ] Demonstrate equivalent platform states after fixed ticks at 1x, scaled,
  and unpaced execution.
- [ ] Run scaled, unpaced, and stepped acceptance tests with `ai-bm-sim`
  absent.
- [x] Validate generated `PositionReportDetailed` through a live Sleet router
  using the pinned UCI 2.5 XSD, then decode the routed wire payload back into
  the generated UCI type. Extend the same live check to other message families
  as their mappings change.
- [x] Publish distinct ownship and wingman `PositionReportDetailed` messages
  through Sleet and verify the production Sensor Models runtime retains both.
- [ ] Demonstrate SuperCell DIS drives Sensor Models deterministically.
- [ ] Demonstrate BMA RoutePlan changes achieved motion only through navigation
  and flight dynamics.
- [ ] Record scenario-time rate, wall throughput, and output-backpressure
  metrics.

## Definition Of Done

SuperCell can run deterministic realtime, scaled, unpaced, and stepped
simulation by itself, and can optionally participate in deterministic lockstep
FTRT ecosystem runs. It publishes complete truth for sensor simulation,
reports cooperating-platform navigation through UCI, and executes BMA plans
without exposing direct position control.

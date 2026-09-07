# SuperCell FTRT DIS/UCI Integration Plan

## Purpose

Define SuperCell's role as the platform-dynamics service in the FTRT
battle-management ecosystem. This plan complements
`docs/faster_than_real_time_development_plan.md` with the cross-service DIS and
UCI responsibilities required by Sensor Models, AST, and BMA.

## Current Ecosystem Role

In the active constructive-simulation path, SuperCell is the platform-dynamics
source. For each authoritative 200 ms scenario tick, it advances the configured
dynamics backend, publishes DIS `EntityStatePdu` truth for sensor-models,
publishes UCI `PositionReportDetailed` navigation state for cooperating blue
platforms, then reports dynamics-stage completion. Sensor-models and AST may
process that same tick only after the completion.

The active message path is:

```text
SuperCell dynamics -> DIS + PositionReportDetailed -> sensor-models -> AST
```

SuperCell owns achieved motion and navigation/autopilot execution; it does not
make battle-management decisions or use DIS truth as an operational BMA input.

### Planning Links

- [Route Planning Development Plan](/home/jeffs/git/route-planning/docs/development_plan.md)
  owns detailed-route production and execution-status feedback.
- [BM Ecosystem Master Plan](/home/jeffs/git/ai-bm-sim/docs/architecture/bm_ecosystem_master_plan.md)
  owns cross-repository ordering and end-to-end acceptance.

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
- [x] Add a selectable adapter that makes the simulation loop follow the
  ecosystem-authoritative tick.
- [x] Add correlated tick acknowledgement after dynamics and required outputs
  complete.
- [ ] Add timeline-epoch reset and branch reinitialization handling. The
  initial adapter rejects an epoch change after advancement rather than
  silently mixing timelines.

### DIS

- [x] Publish DIS `EntityStatePdu` from simulated entities.
- [x] Publish runtime DIS PDUs with deterministic scenario-time conversion;
  retain `current_dis_timestamp()` only for the backward-compatible publisher
  entry point.
- [ ] Add deterministic multi-entity golden-PDU tests.
- [ ] Add lifecycle, stale, and timeline-reset tests.
- [ ] Verify container multicast and unicast operation with Sensor Models.
  - [x] Add an `ai-bm-sim`-selected compose/config path that sends unicast DIS
    to the host-published Sensor Models endpoint. The bounded 2026-08-15 run
    used the checked-out SuperCell binary and delivered valid 144-byte Entity
    State PDUs without Sensor Models framing rejection.

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
- [x] Publish source PRDs on configured ownship and cooperating-platform
  topics with the originating platform's actual `MessageHeader.SystemID`.
  Keep `PositionSource.SubsystemID` as EGI/navigation provenance, not platform
  identity. Validate that topic selection controls recipient delivery; the
  `ai-bm-sim` communications layer will later impose off-platform latency,
  bandwidth, loss, and ordering before forwarding to BMA-visible topics.
  - [x] Populate each source PRD header with the represented platform's
    deterministic `SystemID`, distinct from the EGI `SubsystemID`.
  - [x] Add configured ownship and cooperating-platform source topics and
    topic-selection acceptance coverage.
- [x] Provide the four-friendly-platform ecosystem fixture required by the
  promoted BMA policy, with deterministic platform `SystemID` UUIDs and
  formation-separated JSBSim initial conditions.
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

- [ ] Improve coordinated dynamics runtime efficiency. The 2026-08-29
  SuperCell -> sensor-models -> AST runs averaged about 208 ms wall time in
  SuperCell per 200 ms scenario tick, limiting an attempted 8x run to about
  0.77x effective scenario rate. Profile the per-entity JSBSim/control/output
  path, remove avoidable serial wall-time work, and add an acceptance benchmark
  that reports achieved rate and per-stage wall-time percentiles at 0.25x and
  8x without changing fixed-step scenario results.
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
  - [x] Add the first orchestrated realtime transport topology and remove the
    temporary receiver-state-to-PRD bridge from the active ecosystem service
    set. Sleet authorizes both SuperCell `PositionReport` and
    `PositionReportDetailed` products.
  - [ ] Replace the static integration fixture with an `ai-bm-sim`-derived
    scenario before claiming full deterministic or FTRT system acceptance.
    - [x] Add the SuperCell scenario generator. It consumes the versioned
      `ai-bm-sim` policy-compatible YAML, keeps this repository's transport
      template authoritative, and emits a five-aircraft SuperCell TOML plus
      per-platform JSBSim initial conditions. The generated inputs retain the
      BMA preset and policy provenance rather than duplicating geometry.
    - [x] Have `ai-bm-sim` orchestration invoke the generator and select its
      output for the SuperCell controller and all five JSBSim containers. The
      ecosystem Compose overlay replaces the legacy two-red dependency set
      with one red and four blue FDM instances and mounts every generated
      reset file from the dated run directory.
    - [x] Add a deterministic point-mass `Kinematic` backend for the first
      policy-contract system test. The generated profile preserves the YAML
      initial position, 350/320-knot speeds, and frame-correct headings without
      claiming aerodynamic fidelity; run
      `ai-bm-sim/logs/2026-09-01/184335` remained stable through the first
      four-route BMA publication.
    - [ ] Validate achieved JSBSim initial state, speed, and heading against
      the generated contract. The current bundled C172-derived aliases may not
      attain the promoted policy's requested 350/320-knot kinematics; record
      that result as a compatibility failure rather than silently accepting it.
  - [x] Drive the static fixture from the authoritative ecosystem epoch/ticks.
    The 2026-08-15 lockstep run advanced both control plane and SuperCell to
    exactly 10.8 scenario seconds (54 ticks at 200 ms), emitted 216 DIS PDUs
    for four entities with zero FDM or DIS publication errors, and held both
    clocks unchanged through a six-second wall-time pause.
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


## Display Identity Export (2026-09-06)

The scenario generator now writes `platform-identities.json` version `1.0`
alongside its TOML. Each entry preserves the scenario site instance, producer
SystemID UUID and DIS site/application/entity tuple. Generation and the live
PRD publisher share `owp::system_uuid_for_dis`; consumers must not duplicate
that UUID formula. Ownship retains its configured UUID. ai-bm-sim owns adding
run/epoch and template-local entity references and retaining the artifact hash.
Generator and OWP unit checks passed in the maintained builder container; the
four generated blue UUIDs match the captured 2026-09-02 PRDs. Cross-repository
live display acceptance remains in the ecosystem master plan.

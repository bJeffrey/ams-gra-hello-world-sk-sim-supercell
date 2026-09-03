# Faster-Than-Real-Time UCI Timing

This document describes how SuperCell advances scenario time faster than wall
time while continuing to publish correctly timestamped UCI 2.5 products. It
also distinguishes the time controls that are implemented from timeline rewind
and state restoration, which are not yet implemented in SuperCell.

Line references describe the source as of September 2, 2026. The links point to
the referenced lines so that the implementation can be checked directly.

## Time domains

SuperCell keeps scenario time separate from wall time:

- **Scenario time** is the authoritative time represented by the simulation.
  Dynamics advance it by an exact fixed duration, and it supplies DIS and UCI
  timestamps.
- **Wall time** paces realtime and scaled execution and bounds operational
  concerns such as health checks, transport retries, and optional UCI
  publication-rate protection.

The clock modes and their meanings are defined in
[`src/time.rs`, lines 8-31](../src/time.rs#L8-L31). `ScenarioClock::advance`
adds an exact simulation duration independently of wall-clock elapsed time
([`src/time.rs`, lines 55-68](../src/time.rs#L55-L68)). Consequently, changing
the pacing mode does not change fixed-step dynamics or the state obtained after
the same number of ticks. The cross-mode equivalence test covers realtime,
10x-scaled, unpaced, and stepped execution
([`tests/sim_unit.rs`, lines 1075-1107](../tests/sim_unit.rs#L1075-L1107)).

UCI `MessageModeEnum::Simulation` identifies the messages as simulation data;
it does not select their time rate. SuperCell sets that UCI field while building
the common message header
([`src/owp.rs`, lines 233-241](../src/owp.rs#L233-L241)). Pacing is a simulation
control-plane concern.

## Scaled faster-than-real-time execution

In scaled mode, `rate` is the number of scenario seconds advanced per wall
second. SuperCell computes the wall budget for a tick as:

```text
wall period = fixed scenario step / rate
```

That calculation is implemented in
[`src/time.rs`, lines 77-85](../src/time.rs#L77-L85). For example:

```toml
[time]
mode = "scaled"
rate = 10.0
epoch = "2026-01-01T00:00:00Z"
simulation_hz = 10.0
```

Here the fixed scenario step is 100 ms, but its wall budget is 10 ms. If the
machine sustains that budget, ten seconds of scenario history, including its
UCI products, are produced in approximately one wall-clock second. The
configuration parser maps `mode = "scaled"` and `rate` into `TimeMode::Scaled`
([`src/config.rs`, lines 163-176](../src/config.rs#L163-L176)); a 10x parsing
example is covered in
[`tests/config_contract.rs`, lines 406-434](../tests/config_contract.rs#L406-L434).

The simulation loop calculates a fixed `simulation_dt`, advances the scenario
clock by that value, and sleeps only for any unused wall budget
([`src/sim.rs`, lines 368-380](../src/sim.rs#L368-L380) and
[`src/sim.rs`, lines 951-978](../src/sim.rs#L951-L978)). If processing takes
longer than the scaled wall budget, the runtime records an overrun rather than
changing the scenario step.

## Unpaced execution

**Implemented.** With `mode = "unpaced"`, each loop iteration still advances
exactly one fixed scenario tick, but `ScenarioClock::wall_period_for` returns
`None` ([`src/time.rs`, lines 77-90](../src/time.rs#L77-L90)). The loop therefore
does not enter its wall-period sleep path
([`src/sim.rs`, lines 968-978](../src/sim.rs#L968-L978)). It advances as fast as
dynamics, UCI/DIS publication, and the host can execute.

```toml
[time]
mode = "unpaced"
epoch = "2026-01-01T00:00:00Z"
simulation_hz = 10.0
```

Unpaced does not mean that timestamps use message-arrival time. Each tick first
computes `tick_scenario_time`, publishes state associated with that time, and
then advances the stored clock
([`src/sim.rs`, lines 533-542](../src/sim.rs#L533-L542) and
[`src/sim.rs`, lines 924-954](../src/sim.rs#L924-L954)).

UCI publication frequency is also evaluated against scenario time
([`src/owp.rs`, lines 49-87](../src/owp.rs#L49-L87)). Thus, a configured 1 Hz
`PositionReportDetailed` rate means one report per scenario second even when
scenario seconds pass much faster than wall seconds. The optional
`max_wall_publish_hz` setting is a transport-protection limit: when it is
reached, due batches are coalesced without changing scenario time
([`src/owp.rs`, lines 958-975](../src/owp.rs#L958-L975)).

## Single-tick stepped execution

**Implemented.** With `mode = "stepped"`, the continuous run path refuses to
auto-advance; the caller must use the explicit step API
([`src/sim.rs`, lines 285-309](../src/sim.rs#L285-L309)). `step_once` advances
exactly one fixed tick through the same simulation loop used by the other time
modes ([`src/sim.rs`, lines 342-365](../src/sim.rs#L342-L365)). Because stepped
mode has no wall pacing period, the requested tick completes as soon as its
dynamics and publications complete.

For ecosystem operation, the active configuration selects stepped mode at 5 Hz,
or a 200 ms scenario step
([`config/ai_bm_sim_ecosystem.toml`, lines 13-17](../config/ai_bm_sim_ecosystem.toml#L13-L17)).
The control flow is:

1. `ai-bm-sim` issues a `SimStepRequest` containing `run_id`,
   `timeline_epoch`, `tick_id`, `t_us`, `dt_us`, and `scenario_epoch_utc`.
2. Its SuperCell bridge forwards that tuple to `POST /control/step` and blocks
   until SuperCell finishes the tick and its tick products
   ([`ai-bm-sim/src/app/supercell_dynamics_stage_bridge.py`, lines 34-63](../../ai-bm-sim/src/app/supercell_dynamics_stage_bridge.py#L34-L63)).
3. SuperCell's endpoint waits for the simulation-owning thread to return the
   result ([`src/admin.rs`, lines 183-220](../src/admin.rs#L183-L220)).
4. The main thread validates the next tick and invokes `step_once`
   ([`src/main.rs`, lines 220-270](../src/main.rs#L220-L270)).
5. Only after success does the bridge publish `SimStepCompletion`
   ([`ai-bm-sim/src/app/supercell_dynamics_stage_bridge.py`, lines 76-113](../../ai-bm-sim/src/app/supercell_dynamics_stage_bridge.py#L76-L113)).

This request/completion boundary prevents an orchestrator from advancing the
next stage on the assumption that UDP delivery alone proves a tick is complete.

## UCI fields populated with scenario time

The simulation loop sends each cooperating platform to the OWP publisher as a
`TimedEntityState` containing the state, authoritative scenario timestamp, and
tick ([`src/entity.rs`, lines 206-214](../src/entity.rs#L206-L214) and
[`src/sim.rs`, lines 924-947](../src/sim.rs#L924-L947)). The publisher formats
that scenario timestamp as ISO 8601 and passes the same value to every UCI
builder due on that tick
([`src/owp.rs`, lines 949-1013](../src/owp.rs#L949-L1013)).

The following UCI fields receive that FTRT scenario timestamp:

| UCI product | Scenario-timestamped fields | Implementation |
|---|---|---|
| `PositionReportDetailed` | `MessageHeader.Timestamp` | Common/platform header construction in [`src/owp.rs`, lines 233-257](../src/owp.rs#L233-L257) and PRD use at [`src/owp.rs`, lines 294-297](../src/owp.rs#L294-L297) |
| `PositionReportDetailed` | `MessageData.PositionReportData[].Kinematics.Position.AbsolutePoint.Timestamp` | [`src/owp.rs`, lines 308-318](../src/owp.rs#L308-L318) |
| `PositionReportDetailed` | `MessageData.PositionReportData[].Kinematics.Velocity.Timestamp` | [`src/owp.rs`, lines 320-325](../src/owp.rs#L320-L325) |
| `SystemStatus` | `MessageHeader.Timestamp` | [`src/owp.rs`, lines 356-364](../src/owp.rs#L356-L364) |
| `RoutePlan` | `MessageHeader.Timestamp` | [`src/owp.rs`, lines 509-513](../src/owp.rs#L509-L513) |
| `NavigationReport` | `MessageHeader.Timestamp` | [`src/owp.rs`, lines 545-553](../src/owp.rs#L545-L553) |

Current `RoutePlan` waypoint `Point2D.Timestamp` values are intentionally absent
(`None`), and `RequiredTimeOfArrival` is also absent
([`src/owp.rs`, lines 433-469](../src/owp.rs#L433-L469)). They must not be
described as receiving scenario timestamps in the current implementation.

## Epochs, seeks, and returning to prior state

Two uses of “epoch” must remain distinct:

1. **Scenario UTC epoch:** the absolute RFC 3339 origin. Absolute UCI time is
   `scenario_epoch_utc + scenario elapsed time`. Standalone SuperCell reads this
   from `[time].epoch`; in stepped ecosystem mode it adopts the authoritative
   value from the first accepted step
   ([`src/config.rs`, lines 178-188](../src/config.rs#L178-L188) and
   [`src/main.rs`, lines 249-260](../src/main.rs#L249-L260)).
2. **Timeline epoch:** a generation number used to distinguish records from the
   old continuation from records on a new continuation after a seek or branch.
   It is correlation metadata, not an FDM checkpoint. The external step command
   carries it alongside `run_id`, `tick_id`, and time
   ([`src/admin.rs`, lines 15-29](../src/admin.rs#L15-L29)).

**Returning a running SuperCell instance to a previous state/time is not yet
implemented.** The ecosystem clock can change `t_us`, increment
`timeline_epoch`, reset its tick counter, and publish a branch event
([`ai-bm-sim/src/app/sim_clock.py`, lines 142-171](../../ai-bm-sim/src/app/sim_clock.py#L142-L171)).
SuperCell currently locks onto the first `(run_id, timeline_epoch)` and rejects
any later change with `timeline reset is not supported in this slice`
([`src/main.rs`, lines 274-310](../src/main.rs#L274-L310)). It also requires the
next tick and scenario time to be strictly sequential.

`ScenarioClock::reset` can reset clock bookkeeping
([`src/time.rs`, lines 70-75](../src/time.rs#L70-L75)), and the UCI publication
scheduler resets its deadlines if it observes time moving backward
([`src/owp.rs`, lines 69-81](../src/owp.rs#L69-L81)). Those pieces do not restore
entity or JSBSim state. A correct rewind still requires a checkpoint/replay or
deterministic restart mechanism that restores dynamics, entity state, tick and
elapsed-time counters, timeline identity, and relevant publisher state before
accepting the new epoch. The owning integration plan tracks this remaining work
at [`docs/ftrt_dis_uci_integration_plan.md`, lines 133-135](ftrt_dis_uci_integration_plan.md#L133-L135).

## Current capability summary

| Capability | Status | Behavior |
|---|---|---|
| Scaled FTRT | Implemented | Fixed scenario steps with wall delay divided by `rate` |
| Unpaced | Implemented | Fixed scenario steps with no wall pacing delay |
| Advance exactly one tick | Implemented | Stepped mode through `POST /control/step` / `step_once` |
| Scenario-derived UCI timestamps | Implemented | Fields listed above use the tick's scenario time |
| Initial scenario UTC epoch | Implemented | Configured locally or supplied by the first ecosystem step |
| Timeline-epoch correlation | Partially implemented | Carried and validated for an active timeline |
| Seek/branch metadata in `ai-bm-sim` | Implemented in control plane | Clock increments the epoch and emits a branch event |
| SuperCell rewind/state restoration | Not implemented | Epoch changes are rejected after advancement |

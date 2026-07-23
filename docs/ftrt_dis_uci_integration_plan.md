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

## Required Inputs

- Authoritative `(scenario_time, dt, tick_id, timeline_epoch)` from the
  `ai-bm-sim` simulation control plane.
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
- [ ] Make the simulation loop follow the ecosystem-authoritative tick.
- [ ] Complete scaled, unpaced, and stepped integration.
- [ ] Add tick acknowledgement and timeline-epoch reset handling.

### DIS

- [x] Publish DIS `EntityStatePdu` from simulated entities.
- [ ] Replace `current_dis_timestamp()` with scenario-time conversion.
- [ ] Add deterministic multi-entity golden-PDU tests.
- [ ] Add lifecycle, stale, and timeline-reset tests.
- [ ] Verify container multicast and unicast operation with Sensor Models.

### UCI/CAL

- [x] Publish initial UCI `PositionReport` and `RoutePlan` products.
- [ ] Replace payload `now_utc()` with authoritative scenario timestamps.
- [ ] Replace wall-time publication intervals with scenario-time scheduling.
- [ ] Publish `PositionReportDetailed` for every cooperating platform.
- [ ] Populate navigation covariance from explicit configuration/model data.
- [ ] Consume externally produced `RoutePlan` products.
- [ ] Add plan version, applicability, arrival-time, and rejection tests.

### Acceptance

- [ ] Demonstrate equivalent platform states after fixed ticks at 1x, scaled,
  and unpaced execution.
- [ ] Validate emitted UCI messages against the selected schema version.
- [ ] Demonstrate SuperCell DIS drives Sensor Models deterministically.
- [ ] Demonstrate BMA RoutePlan changes achieved motion only through navigation
  and flight dynamics.
- [ ] Record scenario-time rate, wall throughput, and output-backpressure
  metrics.

## Definition Of Done

SuperCell can participate in deterministic lockstep FTRT runs, publish complete
truth for sensor simulation, report cooperating-platform navigation through
UCI, and execute BMA plans without exposing direct position control.

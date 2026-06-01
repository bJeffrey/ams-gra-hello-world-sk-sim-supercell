# Architecture

## Overview

SuperCell runs a tick-driven simulation loop and publishes DIS `EntityState` PDUs for each active entity. Flying entities are stepped through the JSBSim TCP console. Fixed entities publish static state derived from scenario configuration.

## Components

- **CLI and startup (`src/main.rs`)**
  - Parses `--config`, loads TOML, validates runtime contracts, wires runtime components.
- **Configuration model (`src/config.rs`)**
  - Defines schema for scenario, DIS, FlightGear, JSBSim, and optional OMS LA-CAL settings.
- **Simulation runtime (`src/sim.rs`)**
  - Owns entity lifecycle, waypoint progression, JSBSim stepping, and DIS publish cadence.
  - Handles manual override behavior when FlightGear controls are configured.
- **JSBSim client (`src/fdm.rs`)**
  - Implements line-oriented TCP command/response integration (`set`, `get`, `iterate`).
- **DIS publisher (`src/dis.rs`)**
  - Builds DIS v7 `EntityState` PDUs and sends them over UDP.
- **FlightGear bridge (`src/flightgear.rs`)**
  - Receives `FGNetCtrls` and emits `FGNetFDM` packets for cockpit integration.
- **OWP connection manager (`src/owp.rs`)**
  - Runs a background Tokio runtime for the optional OMS LA-CAL WebSocket connection.
  - Maintains reconnect behavior independently from the simulation loop and stores the latest supplied `EntityState` for OWP publishing logic.
- **Domain state (`src/entity.rs`)**
  - Provides transport-neutral entity state used across FDM, simulation, DIS, and OWP integration.

## Data Flow

1. Startup loads scenario config and validates constraints.
2. Startup evaluates optional OMS LA-CAL configuration; when present, it resolves or generates the `SystemID` and `SubsystemID` via UUIDv5, then starts the background OWP connection manager to maintain the configured WebSocket connection.
3. Runtime constructs entities (flying or fixed) and optional FlightGear bridge.
4. Each tick, flying entities:
   - Optionally ingest FlightGear control input,
   - Write JSBSim setpoints,
   - Step JSBSim,
   - Read current state.
5. Runtime computes derived values (for example DIS dead-reckoning acceleration fields).
6. Runtime publishes one DIS `EntityState` PDU per active entity.
7. The OWP connection manager publishes `mission.position-report`, `mission.system-status`, and `mission.navigation-report` to the LA-CAL WebSocket when configured.

## Related docs

- [Configuration](configuration.md)
- [External interface contracts](contracts.md)
- [Coordinate and unit pipeline](coordinate-pipeline.md)

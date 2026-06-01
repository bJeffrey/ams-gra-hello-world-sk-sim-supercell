# Design Decisions

## Decisions

- **Single runtime state model (`EntityState`)** — Simulation, JSBSim integration, DIS publication, and FlightGear bridge exchange data through a transport-neutral domain struct to keep protocol code isolated from simulation policy.
- **Fail-fast startup for invalid external contracts** — Configuration and JSBSim startup errors terminate process startup instead of silently degrading behavior.
- **Strict config schema at boundary** — Unknown top-level TOML keys are rejected to catch operator mistakes early.
- **Unique DIS identity triplets required** — `(site_id, application_id, entity_id)` must be unique per scenario to avoid ambiguous downstream tracking.
- **`force_id` constrained to DIS core values** — Supported values are `0..=3` (`Other`, `Friendly`, `Opposing`, `Neutral`).
- **FlightGear controls contract is strict** — `FGNetCtrls` packets must match expected version and exact byte size; malformed packets are dropped and logged.
- **Waypoint altitude semantics** — Waypoint `altitude_m` values are treated as MSL metres.
- **DIS publication model** — One `EntityState` PDU is emitted per active entity each tick over UDP.
- **Standard geodetic conversion dependency** — DIS WGS-84 geodetic-to-ECEF conversion uses `map_3d` with its default WGS-84 ellipsoid rather than a project-local formula.
- **Entity failure isolation** — Runtime JSBSim read/write failures mark only the affected flying entity dead; other entities continue.
- **JSBSim read-time stall bound** — Runtime read timeout is intentionally bounded (`2s`) to reduce whole-tick stall amplification.
- **Manual override signaling in DIS appearance** — Air appearance `is_frozen` bit is used to signal manual override active for bridged ownship.
- **Simulation heartbeat watchdog** — Removed local `/tmp/supercell-alive` file; HTTP admin endpoint handles `GET /health`.
- **LA-CAL UUID generation** — `SystemID` and `SubsystemID` are dynamically generated via UUIDv5 by default using names and namespaces, removing the need for manual configuration of raw UUID strings while still allowing overrides.
- **OWP connection isolation** — Ready LA-CAL configuration starts a dedicated background Tokio runtime so WebSocket connection attempts, reconnect delays, and network failures do not block or crash the simulation loop.
- **Latest-state OWP handoff** — The OWP manager receives entity updates through a latest-value watch channel, retaining only the newest `EntityState` instead of queuing every simulation tick.

## Constraints

- SuperCell depends on an external JSBSim process for flying entities.
- DIS and FlightGear protocol transports are UDP and therefore best-effort.
- FlightGear bridge currently targets one configured blue entity.
- UCI LA-CAL message publishing requires dynamic UUID generation or configured UUIDs.
- The OWP WebSocket client currently supports plain `ws://` transport; `wss://` URLs fail connection attempts unless TLS support is enabled in the WebSocket dependency.

## Open Questions

- Whether JSBSim integration should move from synchronous request/response to per-entity asynchronous I/O.
- Whether ignored JSBSim integration tests should be promoted into CI with dedicated runtime services.

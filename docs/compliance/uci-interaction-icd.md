# UCI Interaction ICD: Supercell

## 1. Scope
**Service**: Supercell Simulation Service
**UCI Standard Version**: v2.5

This Interface Control Document (ICD) defines the Universal Command and Control Interface (UCI) messages published and subscribed to by Supercell, following `06_OAC-SPC-002_RevA_UCI_NormalizedInterfaceSpecification_v2_5.docx`.

## 2. Interaction Patterns

### 2.1. System Status Reporting
**Pattern**: Status Reporting (Periodic)
* **Message**: `SystemStatus`
* **Direction**: Published by Supercell
* **Frequency**: Configured via `oms.la-cal.prd_hz`
* **Description**: Reports the operational health, current state, and simulation execution behavior of the Supercell service.
* **Key Fields**:
    * `SystemID`: Configured via `oms.la-cal.system_uuid` or generated via UUIDv5.
    * `SystemState`: Set to `OPERATIONAL`.

### 2.2. Entity Kinematics Publication
**Pattern**: Data Publication
* **Message**: `PositionReport`
* **Direction**: Published by Supercell
* **Frequency**: Configured via `oms.la-cal.position_hz`
* **Description**: Publishes the calculated kinematic state (position, velocity, acceleration, orientation) of simulated entities.
* **Key Fields**:
    * `EntityID`: Unique identifier mapped via DIS triplet (SimulationTargetNumber).
    * `Position`: WGS-84 location.
    * `Velocity`: Cartesian velocity vector.

### 2.3. Navigation Substatus Publication
**Pattern**: Status Reporting
* **Message**: `NavigationReport`
* **Direction**: Published by Supercell
* **Frequency**: Configured via `oms.la-cal.prd_hz`
* **Description**: Encodes navigation subsystem status (`Normal` contingency, `Actual` source) and indicates active navigation capability (`MISSION_PLAN_NAVIGATION` or `MANUAL_NAVIGATION`).

### 2.4. Route Plan Publication
**Pattern**: Data Publication
* **Message**: `RoutePlan`
* **Direction**: Published by Supercell
* **Description**: Only published for entities that have an active mission waypoint sequence.
* **Key Fields**:
    * Encodes the active waypoint segments the simulation is tracking.

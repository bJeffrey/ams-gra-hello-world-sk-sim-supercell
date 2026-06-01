# Coordinate & Unit Pipeline: JSBSim → SuperCell → DIS → Graupel → Worldview

This document traces coordinate frames, units, and transformations from JSBSim state reads through DIS publication and downstream visualization.

For external interface policy (validation, error handling, reliability), see `docs/contracts.md`.

---

## 1. JSBSim properties consumed by SuperCell

`src/fdm.rs::read_state` reads the following batch each tick:

| Property | Units | Frame | Runtime use |
|---|---|---|---|
| `position/lat-geod-rad` | radians | WGS-84 geodetic | `EntityState.latitude_deg` |
| `position/long-gc-rad` | radians | — | `EntityState.longitude_deg` |
| `position/geod-alt-ft` | feet | WGS-84 ellipsoid (HAE) | `EntityState.altitude_m` (DIS geodetic→ECEF altitude input) |
| `position/h-sl-ft` | feet | Mean sea level | `EntityState.altitude_msl_m` (FGNetFDM altitude source) |
| `position/terrain-elevation-asl-ft` | feet | Mean sea level | `EntityState.terrain_elevation_m` (AGL conversion for AP setpoint + FG AGL field) |
| `velocities/v-north-fps` | ft/s | Local NED | `EntityState.velocity_north_mps` |
| `velocities/v-east-fps` | ft/s | Local NED | `EntityState.velocity_east_mps` |
| `velocities/v-down-fps` | ft/s | Local NED | `EntityState.velocity_down_mps` |
| `attitude/phi-rad` | radians | Local NED | `EntityState.roll_deg` |
| `attitude/theta-rad` | radians | Local NED | `EntityState.pitch_deg` |
| `attitude/psi-rad` | radians | Local NED | `EntityState.yaw_deg` |
| `velocities/p-rad_sec` | rad/s | Body axis | `EntityState.roll_rate_rps` |
| `velocities/q-rad_sec` | rad/s | Body axis | `EntityState.pitch_rate_rps` |
| `velocities/r-rad_sec` | rad/s | Body axis | `EntityState.yaw_rate_rps` |

### Not consumed

| Property | Why not consumed |
|---|---|
| `simulation/sim-time-sec` | Intentionally excluded from downstream payload timestamps; however, it is read internally to synchronize TCP stepping in `src/fdm.rs::step`. |
| `position/lat-gc-rad` | DIS position pipeline uses geodetic latitude (`lat-geod-rad`) |
| `position/h-agl-ft` | Runtime derives needed AGL values from MSL altitude and terrain elevation |

---

## 2. SuperCell internal state fields used in boundary contracts

| Field | Units | Frame | Source |
|---|---|---|---|
| `latitude_deg`, `longitude_deg` | degrees | WGS-84 geodetic | JSBSim geodetic radians |
| `altitude_m` | metres | WGS-84 ellipsoid (HAE) | `position/geod-alt-ft` |
| `altitude_msl_m` | metres | Mean sea level | `position/h-sl-ft` |
| `terrain_elevation_m` | metres | Mean sea level | `position/terrain-elevation-asl-ft` |
| `velocity_north/east/down_mps` | m/s | Local NED | JSBSim fps velocities |
| `roll/pitch/yaw_deg` | degrees | Local NED Euler | JSBSim attitude radians |
| `roll/pitch/yaw_rate_rps` | rad/s | Body axis | JSBSim body rates |
| `accel_x/y/z` | m/s² | ECEF | Computed from ECEF velocity deltas in sim loop |

---

## 3. SuperCell → DIS transformation pipeline

### 3.1 Position (geodetic HAE → ECEF)

Input: geodetic lat/lon + `altitude_m` (HAE)  
Output: ECEF `(x,y,z)` in metres.

### 3.2 Velocity (NED → ECEF)

Input: `(v_north, v_east, v_down)` at entity lat/lon  
Output: `(vx, vy, vz)` ECEF in m/s.

### 3.3 Orientation (NED Euler → DIS ECEF Euler)

Input: heading/pitch/roll in local NED  
Output: DIS `psi/theta/phi` in ECEF radians via:
1. `R_ecef_to_ned(lat, lon)`
2. `R_ned_to_body(heading, pitch, roll)`
3. `R_ecef_to_body = R_ned_to_body × R_ecef_to_ned`
4. Euler extraction (`psi`, `theta`, `phi`) from `R_ecef_to_body`

### 3.4 Dead reckoning fields

- Linear acceleration: ECEF velocity delta / `dt`
- Angular velocity: JSBSim body rates passed through
- DR algorithm:
  - Fixed entities: Static
  - Flying entities: DRM_RVW

---

## 4. FlightGear bridge mapping (FGNetFDM)

FGNetFDM uses local-flight conventions distinct from DIS:

| FGNetFDM field | Runtime source |
|---|---|
| `latitude`, `longitude` (rad) | geodetic lat/lon degrees → radians |
| `altitude` (m) | `EntityState.altitude_msl_m` |
| `agl` (m) | `max(altitude_msl_m - terrain_elevation_m, 0.0)` |
| `phi`, `theta`, `psi` (rad) | local NED roll/pitch/yaw degrees → radians |
| `v_north`, `v_east`, `v_down` (ft/s) | NED m/s → ft/s |
| `climb_rate` (ft/s) | `-velocity_down_mps` → ft/s |

FGNetCtrls ingress contract is strict at runtime boundary:
- size must be exactly 744 bytes
- version must be exactly 27
- malformed datagrams are dropped non-fatally

---

## 5. JSBSim autopilot setpoint semantics used by runtime

| Property | Runtime write semantics |
|---|---|
| `ap/altitude_setpoint` | written in feet AGL |
| `ap/heading_setpoint` | true degrees toward active waypoint |
| `ap/altitude_hold`, `ap/heading_hold`, `ap/attitude_hold` | enabled during active control phase |

Waypoint altitude contract is explicit:
- Scenario `[[entities.flight_plan]] altitude_m` is interpreted as **MSL metres**.
- Runtime converts to AGL for JSBSim AP with:
  - `(waypoint_altitude_msl + manual_offset_m - terrain_elevation_m) * M_TO_FT`

---

## 6. Reference frames summary

| Frame | Used for |
|---|---|
| WGS-84 geodetic | Lat/lon source representation |
| ECEF | DIS position, velocity, orientation, linear acceleration |
| Local NED | JSBSim velocity + attitude, FGNetFDM orientation/velocities |
| Body axis | JSBSim and DIS angular velocity |

---

## 7. Remaining accepted deviations

| Issue | Impact | Status |
|---|---|---|
| DIS header timestamp explicitly populated | Yes | `EntityState` PDU headers receive an absolute UTC timestamp computed from system time. Downstream receivers can rely on it for dead-reckoning. |

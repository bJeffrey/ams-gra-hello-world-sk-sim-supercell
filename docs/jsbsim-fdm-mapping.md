# JSBSim → FGNetFDM Property Mapping

This document maps JSBSim properties read in `src/fdm.rs::read_state` to FGNetFDM fields written in `src/flightgear.rs::FgNetFdm::from_entity_state`.

For external interface policy (wire versions, validation, error handling), see `docs/contracts.md`.

## Batch property read set (`fdm.read_state`)

Runtime reads 38 properties per tick via pipelined `get` commands.

| # | JSBSim Property | Runtime field(s) | Notes |
|---|---|---|---|
| 0 | `position/lat-geod-rad` | `latitude_deg` | geodetic radians → degrees |
| 1 | `position/long-gc-rad` | `longitude_deg` | radians → degrees |
| 2 | `position/geod-alt-ft` | `altitude_m` | HAE feet → metres |
| 3 | `position/h-sl-ft` | `altitude_msl_m` | MSL feet → metres |
| 4 | `position/terrain-elevation-asl-ft` | `terrain_elevation_m` | MSL feet → metres |
| 5 | `velocities/v-north-fps` | `velocity_north_mps` | fps → m/s |
| 6 | `velocities/v-east-fps` | `velocity_east_mps` | fps → m/s |
| 7 | `velocities/v-down-fps` | `velocity_down_mps` | fps → m/s |
| 8 | `attitude/phi-rad` | `roll_deg` | radians → degrees |
| 9 | `attitude/theta-rad` | `pitch_deg` | radians → degrees |
| 10 | `attitude/psi-rad` | `yaw_deg` | radians → degrees |
| 11 | `velocities/p-rad_sec` | `roll_rate_rps` | body-axis roll rate |
| 12 | `velocities/q-rad_sec` | `pitch_rate_rps` | body-axis pitch rate |
| 13 | `velocities/r-rad_sec` | `yaw_rate_rps` | body-axis yaw rate |
| 14..37 | engine/aero/fcs/gear properties | corresponding `EntityState` fields | forwarded into FGNetFDM fields where applicable |

`simulation/sim-time-sec` is intentionally not part of this batch read contract.

## FGNetFDM field mapping (`FgNetFdm::from_entity_state`)

| FGNetFDM Field | Source | Units/frame |
|---|---|---|
| `version` | constant | `24` |
| `latitude`, `longitude` | `EntityState.latitude_deg/longitude_deg` | radians, geodetic |
| `altitude` | `EntityState.altitude_msl_m` | metres (MSL) |
| `agl` | `max(altitude_msl_m - terrain_elevation_m, 0.0)` | metres |
| `phi`, `theta`, `psi` | `roll_deg/pitch_deg/yaw_deg` | radians, local NED Euler |
| `v_north`, `v_east`, `v_down` | NED velocities | ft/s |
| `climb_rate` | `-velocity_down_mps` | ft/s |
| `phidot`, `thetadot`, `psidot` | body rates | rad/s |
| `alpha`, `beta` | aero angles | radians |
| `A_X_pilot`, `A_Y_pilot`, `A_Z_pilot` | pilot acceleration fields | ft/s² |
| engine arrays / controls | corresponding engine/FCS fields | protocol-native units |

## Interpolation thread

Runtime also runs an `fg-interp` sender thread at approximately **60 Hz** between simulation ticks.
The thread extrapolates position/heading from latest state and sends best-effort FGNetFDM packets.

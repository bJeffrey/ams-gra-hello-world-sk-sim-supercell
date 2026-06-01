# DIS EntityStatePDU Field Reference

IEEE 1278.1 / DIS v7 — Section 7.2.2

The EntityStatePDU represents the position and state of one entity in the world.
SuperCell publishes one per active entity per tick.

For external interface policy (transport, validation, reliability, and error handling), use `docs/contracts.md` as canonical.
This document focuses on field-level DIS structure and SuperCell mapping details.

---

## PDU Structure Overview

| # | Field | Type | Bytes | SuperCell? | Description |
|---|-------|------|-------|------------|-------------|
| 1 | PDU Header | PduHeader | 12 | ✅ | Protocol version, exercise ID, PDU type, length, timestamp |
| 2 | Entity ID | EntityIdentifier | 6 | ✅ | Unique triplet: site + application + entity |
| 3 | Force ID | enum8 | 1 | ✅ | Affiliation (friendly, opposing, neutral) |
| 4 | Num Variable Parameters | uint8 | 1 | ✅ | Count of variable parameter records |
| 5 | Entity Type | EntityType | 8 | ✅ | What this entity is (aircraft, vehicle, building…) |
| 6 | Alternative Entity Type | EntityType | 8 | ❌ | Disguised/alternative classification |
| 7 | Entity Linear Velocity | Vector3Float | 12 | ✅ | Velocity in ECEF (m/s) |
| 8 | Entity Location | WorldCoordinates | 24 | ✅ | Position in ECEF (m) |
| 9 | Entity Orientation | EulerAngles | 12 | ✅ | ECEF Euler angles (psi/theta/phi in radians) |
| 10 | Entity Appearance | uint32 | 4 | ✅ | Bit flags: power plant (bit 22), AP override (bit 21) |
| 11 | Dead Reckoning Parameters | DrParameters | 40 | ✅ | Algorithm + acceleration + angular velocity |
| 12 | Entity Marking | EntityMarking | 12 | ✅ | 11-char ASCII callsign (e.g. "Eagle-1") |
| 13 | Capabilities | uint32 | 4 | ❌ | Ammunition, fuel supply, recovery, repair |
| 14 | Variable Parameters | VariableParameter[] | 16 each | ❌ | Articulation, attached parts, etc. |

**Total fixed size**: 144 bytes (+ 16 per variable parameter)

---

## Field Details

### 1. PDU Header (12 bytes)

| Field | Type | Bytes | Description |
|-------|------|-------|-------------|
| protocolVersion | enum8 | 1 | `7` = DIS v7 (IEEE 1278.1-2012) |
| exerciseID | uint8 | 1 | Identifies the exercise/simulation |
| pduType | enum8 | 1 | `1` = EntityState |
| protocolFamily | enum8 | 1 | `1` = Entity Information/Interaction |
| timestamp | uint32 | 4 | Relative or absolute timestamp |
| pduLength | uint16 | 2 | Total PDU length in bytes |
| pduStatus | uint8 | 1 | Transfer/LVC/fire type indicators |
| padding | uint8 | 1 | — |

### 2. Entity Identifier (6 bytes)

Unique triplet identifying one entity in the exercise.

| Field | Type | Bytes | Description |
|-------|------|-------|-------------|
| siteID | uint16 | 2 | Facility / installation |
| applicationID | uint16 | 2 | Software application within site |
| entityID | uint16 | 2 | Unique entity within application |

SuperCell config mapping:
```toml
site_id        = 1
application_id = 1
entity_id      = 11
```

### 3. Force ID (1 byte)

| Value | Name | Description |
|-------|------|-------------|
| 0 | Other | Not specified |
| 1 | Friendly | Blue force |
| 2 | Opposing | Red force |
| 3 | Neutral | White/neutral |
| 4–30 | Reserved | — |

SuperCell config: `force_id = 1`

SuperCell validates `force_id` at launch and accepts only `0..=3`
(`Other`, `Friendly`, `Opposing`, `Neutral`). Any other value is treated as
an invalid scenario contract and startup fails with an error.

### 4. Entity Type (8 bytes)

Seven-field enumeration identifying what the entity is.
Defined by SISO-REF-010 (Enumeration and Bit-Encoded Values).

| Field | Type | Bytes | Description |
|-------|------|-------|-------------|
| entityKind | enum8 | 1 | Top-level category |
| domain | enum8 | 1 | Operating domain |
| country | enum16 | 2 | Country of origin |
| category | enum8 | 1 | Category within kind+domain |
| subcategory | enum8 | 1 | Subcategory within category |
| specific | enum8 | 1 | Specific variant |
| extra | enum8 | 1 | Extra discrimination |

#### Entity Kind (UID 7)

| Value | Name | Examples |
|-------|------|----------|
| 0 | Other | — |
| 1 | Platform | Aircraft, vehicle, ship, spacecraft |
| 2 | Munition | Bomb, missile, torpedo |
| 3 | Life Form | Soldier, civilian |
| 4 | Environmental | Cloud, weather cell |
| 5 | Cultural Feature | Building, bridge, road |
| 6 | Supply | Fuel, ammunition, food |
| 7 | Radio | Communication device |
| 8 | Expendable | Chaff, flare, decoy |
| 9 | Sensor/Emitter | Radar, jammer |

#### Domain (UID 8) — for Kind=1 (Platform)

| Value | Name |
|-------|------|
| 0 | Other |
| 1 | Land |
| 2 | Air |
| 3 | Surface (water) |
| 4 | Subsurface |
| 5 | Space |

#### Country (UID 29) — selected values

| Value | Country |
|-------|---------|
| 0 | Other |
| 13 | Australia |
| 29 | Brazil |
| 39 | Canada |
| 45 | China |
| 71 | France |
| 78 | Germany |
| 101 | India |
| 106 | Israel |
| 110 | Japan |
| 134 | South Korea |
| 163 | Norway |
| 170 | Pakistan |
| 180 | Russia |
| 202 | South Africa |
| 224 | United Kingdom |
| 225 | United States |

#### Air Platform Categories (Kind=1, Domain=2) — selected

| Category | Name | Example Subcategories |
|----------|------|-----------------------|
| 1 | Fighter/Air Defense | F-16, F-15, MiG-29 |
| 2 | Attack/Strike | A-10, Su-25 |
| 3 | Bomber | B-52, B-2 |
| 4 | Cargo/Tanker | C-130, KC-135 |
| 5 | ASW/Patrol | P-3, P-8 |
| 6 | Recon | U-2, SR-71 |
| 7 | Electronic Warfare | EA-18G, EC-130 |
| 8 | AWACS/C2 | E-3, E-2 |
| 40 | Trainer | T-38, T-6 |
| 57 | Unmanned | MQ-9, RQ-4 |
| 80 | Helicopter Attack | AH-64, Mi-24 |
| 81 | Helicopter Utility | UH-60, Mi-8 |
| 84 | Civilian Fixed-Wing Single | Cessna 172, Piper |
| 85 | Civilian Fixed-Wing Multi | Boeing 737, A320 |
| 90 | Civilian Helicopter | Bell 206 |

SuperCell config for a C-172:
```toml
[entities.entity_type]
kind        = 1     # Platform
domain      = 2     # Air
country     = 225   # USA
category    = 84    # Civilian Fixed-Wing Single
subcategory = 1     # Cessna 172
specific    = 0
extra       = 0
```

#### Cultural Feature Categories (Kind=5, Domain=1) — selected

| Category | Name |
|----------|------|
| 1 | Building |
| 2 | Bridge |
| 3 | Road |
| 4 | Fence |
| 5 | Tower |
| 7 | Runway |

### 5. Entity Linear Velocity — Vector3Float (12 bytes)

Velocity in Earth-Centered Earth-Fixed (ECEF) coordinates, metres/second.

| Field | Type | Bytes | Description |
|-------|------|-------|-------------|
| x | float32 | 4 | ECEF X velocity (m/s) |
| y | float32 | 4 | ECEF Y velocity (m/s) |
| z | float32 | 4 | ECEF Z velocity (m/s) |

**Note**: ECEF velocity components vary wildly with lat/lon/heading even at
constant ground speed. Receivers must compute `sqrt(x² + y² + z²)` for total
speed, or convert back to local NED using the entity's position.

SuperCell converts NED velocity from JSBSim → ECEF using the standard NED→ECEF
rotation matrix based on entity geodetic position.

### 6. Entity Location — WorldCoordinates (24 bytes)

Position in Earth-Centered Earth-Fixed (ECEF) coordinates, metres.

| Field | Type | Bytes | Description |
|-------|------|-------|-------------|
| x | float64 | 8 | ECEF X position (m) |
| y | float64 | 8 | ECEF Y position (m) |
| z | float64 | 8 | ECEF Z position (m) |

SuperCell converts geodetic (lat/lon/alt WGS-84) → ECEF.

### 7. Entity Orientation — EulerAngles (12 bytes)

Orientation as three successive rotations that transform from the **ECEF
reference frame** to the **entity body frame**.  This is NOT the same as
local NED heading/pitch/roll.

| Field | Type | Bytes | Description |
|-------|------|-------|-------------|
| psi (ψ) | float32 | 4 | Rotation about ECEF Z-axis, from ECEF X-axis (radians) |
| theta (θ) | float32 | 4 | Rotation about intermediate Y-axis (radians) |
| phi (φ) | float32 | 4 | Rotation about body X-axis (radians) |

**Frame conversion**: JSBSim provides heading/pitch/roll in the local NED
frame.  SuperCell converts these to DIS ECEF Euler angles by composing:

1. **R_ecef_to_ned**: rotation from ECEF to NED at the entity's geodetic
   position (lat, lon).
2. **R_ned_to_body**: standard aerospace Euler rotation from NED to body
   frame using heading, pitch, roll.
3. **R_ecef_to_body = R_ned_to_body × R_ecef_to_ned**: combined rotation.
4. DIS Euler angles (psi, theta, phi) are extracted from R_ecef_to_body:
   - `theta = -asin(R[0][2])`
   - `psi = atan2(R[0][1], R[0][0])`
   - `phi = atan2(R[1][2], R[2][2])`

A level aircraft heading north at lat=39°N, lon=105°W produces DIS angles of
approximately psi=75°, theta=-51°, phi=180° — these large values are expected
because the ECEF axes differ significantly from local NED at that position.

### 8. Entity Appearance (4 bytes)

Bit-field flags. Interpretation depends on entity kind and domain.

#### Platform / Air (Kind=1, Domain=2) — UID 31

| Bit(s) | Field | dis-rs name | Values | SuperCell |
|--------|-------|-------------|--------|-----------|
| 0 | Paint Scheme | `paint_scheme` | 0=uniform, 1=camouflage | 0 |
| 1 | Propulsion Kill | `propulsion_killed` | 0=no, 1=yes | 0 |
| 2 | NVG Mode | `nvg_mode` | 0=off, 1=on | 0 |
| 3–4 | Damage | `damage` | 0=none, 1=slight, 2=moderate, 3=destroyed | 0 |
| 5 | Smoke Emanating | `is_smoke_emanating` | 0=no, 1=yes | 0 |
| 6 | Engine Smoke | `is_engine_emitting_smoke` | 0=no, 1=yes | 0 |
| 7–8 | Trailing Effects | `trailing_effects` | 0=none, 1=small, 2=medium, 3=large | 0 |
| 9–11 | Canopy/Troop Door | `canopy_troop_door` | 0=not applicable, 1=closed, 2–4=open states | 0 |
| 12 | Landing Lights | `landing_lights_on` | 0=off, 1=on | 0 |
| 13 | Navigation Lights | `navigation_lights_on` | 0=off, 1=on | 0 |
| 14 | Anti-Collision Lights | `anticollision_lights_on` | 0=off, 1=on | 0 |
| 15 | Flaming | `is_flaming` | 0=no, 1=yes | 0 |
| 16 | Afterburner | `afterburner_on` | 0=off, 1=on | 0 |
| 17 | Lower Anti-Collision Light | `lower_anticollision_light_on` | 0=off, 1=on | 0 |
| 18 | Upper Anti-Collision Light | `upper_anticollision_light_on` | 0=off, 1=on | 0 |
| 19 | Anti-Collision Day/Night | `anticollision_light_day_night` | 0=day, 1=night | 0 |
| 20 | Blinking | `is_blinking` | 0=no, 1=yes | 0 |
| **21** | **Frozen** | **`is_frozen`** | **0=not frozen, 1=frozen** | **⚡ AP override** |
| 22 | Power Plant | `power_plant_on` | 0=off, 1=on | ✅ on for flying |
| 23 | State | `state` | 0=active, 1=deactivated | 0 |
| 24 | Formation Lights | `formation_lights_on` | 0=off, 1=on | 0 |
| 25 | Landing Gear | `landing_gear_extended` | 0=retracted, 1=extended | 0 |
| 26 | Cargo Doors | `cargo_doors_opened` | 0=closed, 1=open | 0 |
| 27 | Nav Position Brightness | `navigation_position_brightness` | 0=dim, 1=bright | 0 |
| 28 | Spot/Search Light 1 | `spot_search_light_1_on` | 0=off, 1=on | 0 |
| 29 | Interior Lights | `interior_lights_on` | 0=off, 1=on | 0 |
| 30 | Reverse Thrust | `reverse_thrust_engaged` | 0=off, 1=on | 0 |
| 31 | Weight on Wheels | `weightonwheels` | 0=no, 1=yes | 0 |

#### SuperCell Appearance Usage

SuperCell sets two bits:

- **Bit 22 (`power_plant_on`)**: Set to `1` for all flying entities, `0` for static/fixed entities.
- **Bit 21 (`is_frozen`)**: **Used as autopilot override / manual control flag.**
  When set to `1`, the ownship pilot has taken manual control via FlightGear
  (the autopilot simulation is "frozen" — not driving the entity). When `0`,
  the autopilot is flying the waypoint route.

This aligns with the DIS semantics of `is_frozen`: the entity's autonomous
simulation behavior (autopilot) is suspended while the pilot flies manually.

### 9. Dead Reckoning Parameters (40 bytes)

Used by receivers to extrapolate entity position and orientation between PDU
updates.

| Field | Type | Bytes | Description |
|-------|------|-------|-------------|
| algorithm | enum8 | 1 | Dead reckoning algorithm |
| otherParameters | uint8[15] | 15 | Algorithm-dependent parameters |
| linearAcceleration | Vector3Float | 12 | ECEF linear acceleration (m/s²) |
| angularVelocity | Vector3Float | 12 | Body-axis angular velocity (rad/s) |

#### Dead Reckoning Algorithm (UID 44)

| Value | Name | Position Extrapolation | Orientation Extrapolation |
|-------|------|------------------------|---------------------------|
| 0 | Other | — | — |
| 1 | Static | None (non-moving) | None |
| 2 | DRM_FPW | P + V×dt | ❌ |
| 3 | DRM_RPW | P + V×dt | θ + ω×dt |
| 4 | DRM_RVW | P + V×dt + ½A×dt² | θ + ω×dt |
| 5 | DRM_FVW | P + V×dt + ½A×dt² | ❌ |
| 6 | DRM_FPB | Same as FPW, body coords | ❌ |
| 7 | DRM_RPB | Same as RPW, body coords | θ + ω×dt |
| 8 | DRM_RVB | Same as RVW, body coords | θ + ω×dt |
| 9 | DRM_FVB | Same as FVW, body coords | ❌ |

SuperCell sets the DR algorithm based on scenario entity kind:

| Entity State | Algorithm | Fields Populated |
|---|---|---|
| Fixed (configured static entity) | **Static (1)** | None — all DR fields zero |
| Flying (runtime-stepped entity) | **DRM_RVW (4)** | linearAcceleration + angularVelocity |

For DRM_RVW, SuperCell populates:

- **linearAcceleration**: ECEF linear acceleration (m/s²), computed from the
  ECEF velocity delta between successive ticks divided by dt.
- **angularVelocity**: body-axis angular rates from JSBSim (rad/s):
  - x = roll rate (p)
  - y = pitch rate (q)
  - z = yaw rate (r)

This allows receivers to extrapolate between 2 Hz updates:
- Position: `P = P₀ + V₀×dt + ½×A₀×dt²`
- Orientation: `θ = θ₀ + ω×dt`

### 10. Entity Marking (12 bytes)

Human-readable callsign displayed on the entity.

| Field | Type | Bytes | Description |
|-------|------|-------|-------------|
| characterSet | enum8 | 1 | `1` = ASCII |
| characters | char[11] | 11 | Null-padded ASCII string |

Examples: `"Eagle-1"`, `"Bandit-1"`, `"KVUU-99.9"`

SuperCell config: uses the `name` field from each entity.
```toml
name = "Eagle-1"    # → marking "Eagle-1" (7 chars)
name = "Bandit-1"   # → marking "Bandit-1" (8 chars)
name = "KVUU-99.9"  # → marking "KVUU-99.9" (9 chars)
```

Maximum 11 characters. Longer names are truncated.

Markings are encoded as ASCII. Non-ASCII characters are removed before
truncation so encoding is non-panicking and wire-safe.

### 11. Capabilities (4 bytes)

Bit-field indicating entity capabilities.

| Bit | Capability |
|-----|------------|
| 0 | Ammunition supply |
| 1 | Fuel supply |
| 2 | Recovery |
| 3 | Repair |
| 4–31 | Reserved |

SuperCell currently sets: `0`.

### 12. Variable Parameters (16 bytes each)

Optional records for articulation (gear, turrets, flaps), attached parts, or
entity-type extensions.

| Field | Type | Bytes | Description |
|-------|------|-------|-------------|
| recordType | enum8 | 1 | 0=articulation, 1=attached part, 2=entity-type-VP |
| fields | varies | 15 | Content depends on recordType |

SuperCell currently sets: none (empty list).

---

## SuperCell Implementation Status

| Field | Status | Notes |
|-------|--------|-------|
| PDU Header | ✅ Complete | v7, exercise ID from config, absolute timestamp from system clock |
| Entity ID | ✅ Complete | site/app/entity from config |
| Force ID | ✅ Complete | 0..=3 validated at startup; invalid values fail launch |
| Entity Type | ✅ Complete | Full 7-field type from config |
| Alternative Entity Type | ❌ Not set | Zeros |
| Linear Velocity | ✅ Complete | NED→ECEF rotation matrix |
| Location | ✅ Complete | Geodetic→ECEF (WGS-84) |
| Orientation | ✅ Complete | NED Euler→ECEF Euler via full rotation matrix composition |
| Appearance | ✅ Partial | power_plant_on for flying; is_frozen = manual override flag |
| Dead Reckoning | ✅ Complete | Static for configured fixed entities; DRM_RVW with ECEF accel + body-axis angular vel for flying entities |
| Marking | ✅ Complete | Entity `name` from config, ASCII-sanitized then truncated to 11 chars |
| Capabilities | ❌ Not set | 0 |
| Variable Parameters | ❌ Not set | Empty |

---

## Coordinate Transformations

SuperCell performs five coordinate transformations when building each PDU:

| Source (JSBSim/config) | Target (DIS PDU) | Transformation |
|---|---|---|
| Geodetic lat/lon/alt (WGS-84) | ECEF position (m) | Standard geodetic→ECEF |
| NED velocity (m/s) | ECEF velocity (m/s) | NED→ECEF rotation matrix at entity position |
| NED heading/pitch/roll (deg) | ECEF Euler angles (rad) | R_ecef_to_body = R_ned_to_body × R_ecef_to_ned, then extract Euler angles |
| ECEF velocity delta between ticks | ECEF acceleration (m/s²) | (V_current − V_previous) / dt |
| JSBSim body-axis p/q/r (rad/s) | Body-axis angular velocity (rad/s) | Direct pass-through (both are body frame) |

### Reference Frames

**ECEF (Earth-Centered Earth-Fixed)**
- Origin at Earth's center of mass
- X axis: intersection of prime meridian and equator
- Y axis: 90° east longitude at equator
- Z axis: north pole
- Used for: `entityLocation`, `entityLinearVelocity`, `linearAcceleration`, `entityOrientation`

**WGS-84 (World Geodetic System 1984)**
- Semi-major axis: 6,378,137.0 m
- Flattening: 1/298.257223563
- SuperCell converts geodetic (lat/lon/alt) → ECEF for DIS

**NED (North-East-Down)**
- Local tangent plane at entity position
- North: geodetic north, East: geodetic east, Down: toward Earth center
- Used internally by JSBSim for velocity and attitude
- SuperCell converts to ECEF for DIS

**Body Frame**
- Origin at entity center of mass
- X axis: forward (nose), Y axis: right (starboard), Z axis: down (belly)
- Used for: angular velocity (p, q, r) — passed directly to DIS without conversion

### DIS Euler Angle Convention

The DIS orientation Euler angles (psi, theta, phi) define three successive
rotations that transform from the ECEF frame to the entity body frame:

1. **psi (ψ)**: rotation about the ECEF Z-axis
2. **theta (θ)**: rotation about the intermediate Y-axis
3. **phi (φ)**: rotation about the body X-axis

These are **not** the same as local NED heading/pitch/roll.  A level aircraft
heading north at mid-latitudes will have large DIS Euler angles because the
ECEF axes differ significantly from local NED.  SuperCell performs the full
rotation matrix composition to convert correctly.

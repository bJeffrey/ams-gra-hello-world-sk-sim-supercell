# OMS Service Contract Addendum: Supercell

## 1. Scope
**Service**: Supercell Simulation Service
**GRA Version**: [TBD]
**OMS Standard**: v2.5

This document serves as the OMS Service Contract Addendum for the Supercell service, mapping Agile Mission Suite (AMS) Government Reference Architecture (GRA) requirements to the OMS implementation details as required by GRA-MPU-002.

## 2. Service Description
Supercell provides core simulation capabilities (kinematics, physical models, entity state tracking) within the Agile Mission Suite (AMS) mission system architecture. It operates as an AMS GRA compatible service running on a compliant Mission Processing Unit (MPU).

## 3. AMS GRA Interfaces & Openness
* **Transport**: Supercell utilizes the Abstract Service Bus (ASB) provided by the hosting platform, accessed via the OMS Critical Abstraction Layer (CAL). This interface is **Open**.
* **Message Exchange Layer (MEL)**: N/A - Supercell does not have a MEL interface.
* **DIS Interface**: Standard DIS protocol over UDP. This interface is **Open**.

## 4. OMS Addendum

### 4.1. CAL Binding
Supercell utilizes the Sleet Language-Agnostic CAL (LA-CAL) binding over a WebSocket OWP connection (`oms.la-cal.ws_url`) for all ASB interactions.

### 4.2. Subsystem Abstraction
As a software service, Supercell does not directly interface with hardware subsystems. If hardware acceleration (e.g., GPU for physics) is required, it is abstracted via the OS Facade.

### 4.3. Data Exchange Constraints
* Delivery reliability: Handled by the underlying TCP/WebSocket layer for LA-CAL communication; UDP for external DIS publishing (best-effort delivery).
* Payload size: DIS packets conform to standard network MTU sizes.

## 5. Security Addendum

### 5.1. Controls & Risks
* **Compute Constraints**: Memory and CPU use scales with the number of entities. A new container is spawned for each moving entity.
* **Security Constraints**: Outbound UCI messages are marked Unclassified (U) and USA. These security markings (`classification` and `owner_producer`) are exposed as configuration items that can be changed.
* **Supply Chain Risks**: Supercell relies on standard Rust crates mapped in `Cargo.lock`. Supply chain analysis is performed via `cargo deny`.

### 5.2. Application Security Testing
* **Static Application Security Analysis (SAST)**: Enforced via `cargo clippy`, `cargo audit`, and `cargo deny` during the CI pipeline.
* **Dynamic Application Security Testing (DAST)**: [TBD - DAST methodology to be integrated]

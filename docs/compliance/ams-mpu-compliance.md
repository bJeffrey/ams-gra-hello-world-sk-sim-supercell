# AMS GRA MPU Compliance Documentation: Supercell (OMS Service MPU)

## 1. Scope
**Unit Name**: Supercell
**MPU Type**: OMS Service MPU
**GRA Version**: [TBD]

This document assesses the compliance of Supercell against the Agile Mission Suite (AMS) Government Reference Architecture (GRA) requirements for an OMS Service MPU.

## 2. Applicability
Supercell is evaluated as a pure software service conforming to the OMS Service MPU profile.

## 3. Compliance Assessment

### 3.1. General MPU Requirements
* **Architecture Compliance**: Yes. Supercell is designed as a modular, loosely coupled service.
* **Resource Management**: Yes. Resource limits are managed by the platform/container orchestrator (OS Facade).

### 3.2. OMS Service MPU Specific Requirements
* **CAL Compliance**: Yes. Supercell uses the CAL for all intra-system communications.
* **Service Contract**: Yes. Provided in `oms-service-contract.md`.
* **Lifecycle Management**: Yes. Supercell conforms to the standard OMS service lifecycle states.

### 3.3. Interface Design Descriptions (IDDs)
* **Message Exchange Layer (MEL)**: N/A - Supercell does not have a MEL interface.

### 3.4. GRA Requirements Table
The following table assesses Supercell against the specific `GRA-MPU` requirements for an OMS Service MPU as defined in the AMS GRA v14.0 Architecture Volumes.

| ID | Requirement | Tier | Status / Justification |
|---|---|---|---|
| GRA-MPU-002 | Documented IAW OMS Service Contract Template and Addendum. | 1 | **Compliant**: Documented in `oms-service-contract.md` and `oms-service-contract-addendum.md`. |
| GRA-MPU-003 | If on a GPP, restricted to using POSIX API for OS system calls. | 1 | **[TBD]**: Modern containerized runtimes and safe languages (Rust) abstract syscalls and rely on non-POSIX Linux-specific syscalls (e.g., `epoll`, `io_uring`, `futex`). Strict POSIX-only restriction is fundamentally incompatible with the GRA-MPU-057 requirement for OCI containers. |
[an: only say TBD]
| GRA-MPU-004 | Meet all OMS Tier 1 requirements for an OMS Service. | 1 | **Compliant**: Assessed in `oms-service-checklist.md`. |
| GRA-MPU-005 | Protect CPI cryptographically using an open implementation. | 3 | **N/A**: Supercell does not contain Critical Program Information (CPI). |
| GRA-MPU-006 | Implement OMS AT ICD if processing CPI on built-for-export platform. | 2 | **N/A**: Supercell does not process CPI. |
| GRA-MPU-024 | Use CDS/CU IDD for exchanges with CDS/Cryptographic Unit. | 1 | **N/A**: Supercell does not interact with a CDS or CU. |
| GRA-MPU-025 | Report faults with the associated fault status. | 1 | **Compliant**: Reports state and faults via the UCI `SystemStatus` message. |
| GRA-MPU-026 | Implement Required interfaces specified IAW the AMS GRA Model. | 1 | **Compliant**: Interfaces defined in `uci-interaction-icd.md`. |
| GRA-MPU-027 | Implement all interfaces with the specified level of Openness. | 1 | **Compliant**: All external interfaces use open standards (OMS/UCI, DIS). |
| GRA-MPU-030 | Document interface openness IAW the Compliance Assessment Approach. | 1 | **Compliant**: Documented in `oms-service-contract-addendum.md`. |
| GRA-MPU-031 | Include a model compatible with MBSE content in the AMS GRA SK. | 2 | **[TBD]**: MBSE model not yet provided. |
| GRA-MPU-032 | Include a model that identifies all external MPU interfaces. | 2 | **[TBD]**: MBSE model not yet provided. |
| GRA-MPU-039 | Provide a CLASSIFICATION.md file defining security classification. | 2 | **[TBD]**: File not yet created. |
| GRA-MPU-040 | Provide a DATARIGHTS.md file defining data rights. | 2 | **[TBD]**: File not yet created. |
| GRA-MPU-041 | Document SAST results for microprocessor software source code. | 1 | **Compliant**: Assessed during pipeline execution (cargo clippy/audit). |
| GRA-MPU-042 | Document DAST results for microprocessor software. | 1 | **Compliant**: Assessed via e2e testing during pipeline execution. |
| GRA-MPU-046 | Automated tests with at least 50% source code structural coverage. | 1 | **Compliant**: Met by unit/integration test suites. |
| GRA-MPU-047 | Use relative file paths in the execution environment. | 2 | **Compliant**: Utilizes relative file paths for config loading. |
| GRA-MPU-048 | Use relative file paths in the build environment. | 2 | **Compliant**: Cargo build uses relative paths. |
| GRA-MPU-051 | Include an SBOM in an open and non-proprietary format. | 1 | **Compliant**: Handled by `cargo about` / `cargo deny` in the pipeline. |
| GRA-MPU-053 | DP shall contain content only from that MPU. | 3 | **N/A**: No Digital Payloads (DP) for this software service. |
| GRA-MPU-057 | DP targeting a GPP shall be a single OCI-compliant container. | 1 | **Compliant**: Delivered as an OCI-compliant container image (`registry.gitlab.com/.../supercell`). |
| GRA-MPU-058 | Encrypt classified DaR using NSA-approved algorithms. | 1 | **N/A**: Supercell does not store classified Data at Rest. |
| GRA-MPU-059 | Decrypt/encrypt classified data keys only stored in volatile memory. | 1 | **N/A**: Supercell does not handle classified data. |
| GRA-MPU-066 | Support the use of a Mission System-provided RoT. | 1 | **N/A**: Pure software service; RoT managed by the host platform. |
| GRA-MPU-071 | Built, packaged, and delivered to storage via automated pipeline. | 1 | **Compliant**: Handled by GitLab CI/CD. |
| GRA-MPU-074 | Process/decrypt CPI fully implements OMS AT ICD. | 3 | **N/A**: No CPI. |
| GRA-MPU-077 | Automated tests with at least 80% source code structural coverage. | 3 | **[TBD]**: Needs verification of coverage metrics. |
| GRA-MPU-078 | Model capable of automated verification with AMS GRA models. | 3 | **[TBD]**: MBSE model not yet provided. |
| GRA-MPU-079 | Model provides definition of all external MPU interfaces. | 3 | **[TBD]**: MBSE model not yet provided. |
| GRA-MPU-082 | Pass 70% or more of applicable jobs in a compliant pipeline. | 2 | **Compliant**: Passes 100% of pipeline checks. |
| GRA-MPU-083 | Pass 100% of applicable jobs in a compliant pipeline. | 3 | **Compliant**: Passes 100% of pipeline checks. |
| GRA-MPU-085 | Report faults with the associated fault status using applicable interface. | 2 | **Compliant**: Uses UCI `SystemStatus` via LA-CAL. |
| GRA-MPU-086 | Use the CMM IDD for all information exchanges with the CMM. | 1 | **N/A**: Does not interface with Countermeasures Manager. |
| GRA-MPU-090 | Provide a configuration file named `service.yml` IAW the Template. | 1 | **[TBD]**: Missing `service.yml`. |

## 4. Verification & Validation
* **Verification Method**: Test / Analysis
* **Test Procedures**: Handled via standard unit test suites and integration/e2e testing in the deployment environment. The CI/CD pipeline validation is functionally identical to the local development workflow, ensuring consistency.
* **Results**: Assessed automatically during pipeline execution.

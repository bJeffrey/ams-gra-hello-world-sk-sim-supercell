# OMS Service Checklist: Supercell

## Overview
This checklist documents the compliance of the Supercell service against the OMS v2.5 standard, following `18_1_OMSC-CHK-005_RevM_ServiceChecklist_DandD_v2_5.xlsx`.

## Service Details
* **Service Name**: Supercell Simulation Service
* **Version**: 1.0.0
* **Target OMS Tier**: Tier 3 (Full Interoperability)

## Checklist Items

| Req ID | Requirement Description | Compliance (Yes/No/NA) | Justification / Evidence |
|---|---|---|---|
| SRV-001 | Does the service adhere to the CAL for all intra-mission package communications? | Yes | Utilizes the Sleet LA-CAL binding (WebSocket OWP) for all publish interactions. |
| SRV-002 | Does the service start, initialize, run, and shutdown using standard CAL lifecycle hooks? | Yes | Implemented via process supervision and LA-CAL connection lifecycle. |
| SRV-003 | Does the service provide a documented Service Contract? | Yes | See `oms-service-contract.md`. |
| SRV-004 | Are all consumed and produced messages defined in the Service Contract? | Yes | Detailed in Interface section of the contract. |
| SRV-005 | Does the service publish periodic health/status via UCI `SystemStatus`? | Yes | Published at the `oms.la-cal.prd_hz` configured rate. |
| SRV-006 | Does the service avoid using direct point-to-point IP addressing for mission data? | Yes | All UCI mission data traverses the LA-CAL WebSocket router. |
| SRV-007 | Do outbound UCI messages identify their mode appropriately? | Yes | All outbound UCI messages have `MessageHeader.Mode` explicitly set to `SIMULATION`. |

## Assessment Summary
* **Status**: Self-Assessed
* **Assessor**: Supercell Maintainers
* **Date**: (Current build)

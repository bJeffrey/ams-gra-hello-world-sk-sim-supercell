//! SuperCell — a DIS simulator that steps fixed and JSBSim-backed entities
//! and publishes `EntityState` PDUs over UDP.
//!
//! See the [README](../README.md) and [`docs/`](../docs/) for architecture,
//! configuration, and external interface contracts.

/// Admin server for metrics and health checks.
pub mod admin;
/// Scenario configuration model and validation.
pub mod config;
/// DIS PDU construction, coordinate conversion, and UDP publication.
pub mod dis;
/// Transport-neutral entity state and type definitions.
pub mod entity;
/// JSBSim TCP console client and FDM adapter.
pub mod fdm;
/// FlightGear protocol structs and UDP bridge.
pub mod flightgear;
/// OMS LA-CAL OWP connection management.
pub mod owp;
/// Tick-driven simulation runtime.
pub mod sim;
/// Telemetry, logging, and metrics infrastructure.
pub mod telemetry;

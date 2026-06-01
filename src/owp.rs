//! Manage the OMS LA-CAL OWP WebSocket connection.
//!
//! The manager owns a background Tokio runtime so WebSocket I/O cannot block the
//! synchronous simulation loop. It keeps the latest entity state available for
//! publishing logic while connection establishment and reconnect behavior run in
//! the background.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::runtime::Builder;
use tokio::sync::watch;
use tracing::{debug, error, info, warn};

use sleet_client::{CalClient, InitOptions};
use sleet_types::uci::v2_5::{
    AltitudeReferenceEnum, ClassificationEnum, EndPointType, EnduranceType, EnvironmentEnum,
    HeaderType, InertialStateType, LineProjectionEnum, MessageModeEnum, NavigationCapabilityEnum,
    NavigationReportMdt, NavigationReportMt, NavigationSourceDetailsType, OwnerProducerChoiceType,
    OwnerProducerEnum, PathIdType, PathSegmentSourceEnum, PathSegmentType, PathTypeEnum,
    PlanApplicabilityType, Point2DType, Point4DType, PositionReportMdt, PositionReportMt,
    RoutePathType, RoutePlanIdType, RoutePlanMdt, RoutePlanMt, RoutePlanPartsType, RoutePlanType,
    RouteType, SecurityInformationType, SegmentIdType, SubsystemIdType, SystemContingencyLevelEnum,
    SystemIdType, SystemSourceEnum, SystemStateEnum, SystemStatusMdt, SystemStatusMt,
    Velocity3DType, WayPointPointChoiceType, WayPointType, WaypointTypeEnum,
};

use crate::config::LaCalConfig;
use crate::entity::EntityState;

const DEFAULT_INITIAL_RETRY_DELAY: Duration = Duration::from_secs(1);
const DEFAULT_MAX_RETRY_DELAY: Duration = Duration::from_secs(5);

#[derive(serde::Serialize)]
struct PositionReportWrapper {
    #[serde(rename = "PositionReport")]
    position_report: PositionReportMt,
}

#[derive(serde::Serialize)]
struct SystemStatusWrapper {
    #[serde(rename = "SystemStatus")]
    system_status: SystemStatusMt,
}

#[derive(serde::Serialize)]
struct RoutePlanWrapper {
    #[serde(rename = "RoutePlan")]
    route_plan: RoutePlanMt,
}

#[derive(serde::Serialize)]
struct NavigationReportWrapper {
    #[serde(rename = "NavigationReport")]
    navigation_report: NavigationReportMt,
}

fn build_system_id(uuid: uuid::Uuid) -> SystemIdType {
    SystemIdType {
        uuid: uuid.to_string(),
        descriptive_label: None,
    }
}

fn build_mission_id(uuid: uuid::Uuid) -> sleet_types::uci::v2_5::MissionIdType {
    sleet_types::uci::v2_5::MissionIdType {
        uuid: uuid.to_string(),
        descriptive_label: None,
        version: None,
    }
}

fn build_security_info(config: &OwpPublisherConfig) -> SecurityInformationType {
    SecurityInformationType {
        classification: config.classification().clone(),
        owner_producer: vec![OwnerProducerChoiceType::GovernmentIdentifier {
            government_identifier: config.owner_producer().clone(),
        }],
        joint: None,
        sci_controls: vec![],
        sar_identifier: vec![],
        atomic_energy_markings: vec![],
        dissemination_controls: vec![],
        display_only_to: vec![],
        fgi_source_open: vec![],
        fgi_source_protected: vec![],
        releasable_to: vec![],
        non_ic_markings: vec![],
        classified_by: None,
        compilation_reason: None,
        derivatively_classified_by: None,
        classification_reason: None,
        non_us_controls: vec![],
        derived_from: None,
        declass_date: None,
        declass_event: None,
        declass_exception: vec![],
        has_approximate_markings: None,
        high_water_nato: vec![],
        cui_basic: vec![],
        cui_specified: vec![],
        cui_decontrol_date: None,
        cui_decontrol_event: None,
        cui_controlled_by: None,
        cui_controlled_by_office: None,
        cui_poc: None,
        handle_via_channels: None,
        second_banner_line: vec![],
    }
}

fn build_header(config: &OwpPublisherConfig, timestamp: &str) -> HeaderType {
    HeaderType {
        system_id: build_system_id(config.system_uuid()),
        timestamp: timestamp.to_string(),
        schema_version: "002.5.0".to_string(),
        mode: MessageModeEnum::Simulation,
        service_id: None,
        mission_id: Some(build_mission_id(config.mission_uuid())),
    }
}

fn build_position_report(
    state: &EntityState,
    config: &OwpPublisherConfig,
    timestamp: &str,
) -> PositionReportWrapper {
    let mt = PositionReportMt {
        security_information: build_security_info(config),
        message_header: build_header(config, timestamp),
        message_data: PositionReportMdt {
            system_id: build_system_id(config.system_uuid()),
            display_name: Some(state.marking.clone()),
            source: SystemSourceEnum::Actual,
            current_operating_domain: EnvironmentEnum::Air,
            inertial_state: InertialStateType {
                position: Point4DType {
                    latitude: state.latitude_deg.to_radians(),
                    longitude: state.longitude_deg.to_radians(),
                    altitude: state.altitude_m,
                    altitude_reference: Some(AltitudeReferenceEnum::WgsHae),
                    timestamp: timestamp.to_string(),
                    depth_category: None,
                    hae_adjustment: None,
                },
                position_uncertainty: None,
                domain_velocity: Some(Velocity3DType {
                    north_speed: state.velocity_north_mps,
                    east_speed: state.velocity_east_mps,
                    down_speed: state.velocity_down_mps,
                    timestamp: Some(timestamp.to_string()),
                }),
                ground_velocity: None,
                domain_acceleration: None,
                link16_position_quality: None,
                orientation: None,
                orientation_rate: None,
            },
            wander_angle: None,
            magnetic_heading: None,
            timestamp: Some(timestamp.to_string()),
            simulation_target_number: Some(
                ((state.site_id as i64) << 32)
                    | ((state.application_id as i64) << 16)
                    | (state.entity_id as i64),
            ),
        },
    };
    PositionReportWrapper {
        position_report: mt,
    }
}

fn build_system_status(
    config: &OwpPublisherConfig,
    timestamp: &str,
    is_ready: bool,
) -> SystemStatusWrapper {
    let mt = SystemStatusMt {
        security_information: build_security_info(config),
        message_header: build_header(config, timestamp),
        message_data: SystemStatusMdt {
            system_id: build_system_id(config.system_uuid()),
            system_state: if is_ready {
                SystemStateEnum::Operational
            } else {
                SystemStateEnum::Inactive
            },
            system_state_reason: vec![],
            predicted_system_state: vec![],
            source: SystemSourceEnum::Actual,
            fusion_eligibility: None,
            model: None,
            identity: None,
            communications: None,
            operator: vec![],
            subsystem_id: vec![SubsystemIdType {
                uuid: config.subsystem_uuid().to_string(),
                descriptive_label: None,
            }],
            capability_id: vec![],
            service_id: vec![],
            platform_status: None,
            voice_control: None,
            activity_by: vec![],
            strength: None,
        },
    };
    SystemStatusWrapper { system_status: mt }
}

fn build_route_plan(
    state: &EntityState,
    config: &OwpPublisherConfig,
    timestamp: &str,
) -> RoutePlanWrapper {
    let route_plan_uuid = {
        let mut bytes = config.system_uuid().into_bytes();
        bytes[0] ^= 0xAA;
        uuid::Uuid::from_bytes(bytes)
    };

    let mut seg_uuids = Vec::with_capacity(state.waypoints.len());
    for i in 0..state.waypoints.len() {
        let mut bytes = route_plan_uuid.into_bytes();
        let i_bytes = (i as u32).to_le_bytes();
        // XOR into the last 4 bytes to avoid modifying version/variant fields
        bytes[12] ^= i_bytes[0] ^ 0xBB;
        bytes[13] ^= i_bytes[1];
        bytes[14] ^= i_bytes[2];
        bytes[15] ^= i_bytes[3];
        seg_uuids.push(uuid::Uuid::from_bytes(bytes));
    }

    let mut path_segments = Vec::with_capacity(state.waypoints.len());
    for (i, wp) in state.waypoints.iter().enumerate() {
        let seg_uuid = seg_uuids[i];
        let next_path_segment = if i + 1 < state.waypoints.len() {
            Some(sleet_types::uci::v2_5::NextPathSegmentType {
                path_id: None,
                path_segment_id: SegmentIdType {
                    uuid: seg_uuids[i + 1].to_string(),
                    descriptive_label: None,
                    version: None,
                },
            })
        } else {
            None
        };

        let end_point = EndPointType::WayPoint {
            way_point: WayPointType {
                point_choice: WayPointPointChoiceType::Point2D {
                    point2_d: Point2DType {
                        latitude: wp.latitude_deg.to_radians(),
                        longitude: wp.longitude_deg.to_radians(),
                        altitude: Some(wp.altitude_m),
                        altitude_range: None,
                        altitude_reference: Some(AltitudeReferenceEnum::Msl),
                        timestamp: None,
                    },
                },
                waypoint_type: WaypointTypeEnum::NavOnly,
                dmpi_point_id: None,
            },
        };

        path_segments.push(PathSegmentType {
            path_segment_id: SegmentIdType {
                uuid: seg_uuid.to_string(),
                descriptive_label: None,
                version: None,
            },
            source: PathSegmentSourceEnum::OperatorDefined,
            end_point,
            locked: None,
            modified: None,
            speed: None,
            civil_path_terminator: None,
            climb: None,
            maximum_roll: None,
            acceleration: None,
            next_path_segment,
            conditional_path_segment: vec![],
            inertial_state: vec![],
            required_time_of_arrival: None,
            remarks: None,
            required_navigation_performance_in_meters: None,
            fix_identifier: None,
        });
    }

    let path_uuid = {
        let mut bytes = route_plan_uuid.into_bytes();
        bytes[2] ^= 0xCC;
        uuid::Uuid::from_bytes(bytes)
    };
    let default_seg_id = SegmentIdType {
        uuid: route_plan_uuid.to_string(), // Just fallback to route_plan_uuid to be deterministic
        descriptive_label: None,
        version: None,
    };

    let first_in_path_segment_id = if let Some(first) = path_segments.first() {
        first.path_segment_id.clone()
    } else {
        default_seg_id
    };

    let path = RoutePathType {
        path_id: PathIdType {
            uuid: path_uuid.to_string(),
            descriptive_label: None,
            version: None,
        },
        path_type: PathTypeEnum::Primary,
        first_in_path_segment_id,
        path_segment: path_segments,
        initial_conditions: None,
        airfield_id: None,
        runway_id: None,
        remarks: None,
    };

    let path_id_clone = path.path_id.clone();

    let mt = RoutePlanMt {
        security_information: build_security_info(config),
        message_header: build_header(config, timestamp),
        object_state: None,
        message_data: RoutePlanMdt {
            route_plan_id: RoutePlanIdType {
                uuid: route_plan_uuid.to_string(),
                descriptive_label: None,
                version: None,
            },
            plan_command_id: None,
            plan: RoutePlanType {
                applicability: PlanApplicabilityType {
                    planned_for_id: None,
                    applicable_to_id: build_system_id(config.system_uuid()),
                },
                window: None,
                parts: RoutePlanPartsType {
                    route_type: vec![PathTypeEnum::Primary],
                },
                route: RouteType {
                    detailed: false,
                    first_in_route_path_id: path_id_clone,
                    route_projection: LineProjectionEnum::GreatCircle,
                    path: vec![path],
                    remarks: None,
                },
            },
            for_planning_use_only: false,
            plan_inputs: None,
        },
    };

    RoutePlanWrapper { route_plan: mt }
}

fn build_navigation_report(
    state: &EntityState,
    config: &OwpPublisherConfig,
    timestamp: &str,
) -> NavigationReportWrapper {
    let mt = NavigationReportMt {
        security_information: build_security_info(config),
        message_header: build_header(config, timestamp),
        message_data: NavigationReportMdt {
            system_id: build_system_id(config.system_uuid()),
            source: SystemSourceEnum::Actual,
            contingency_level: SystemContingencyLevelEnum::Normal,
            endurance: EnduranceType {
                fuel: None,
                duration: None,
                duration_end: None,
                percent: None,
                footprint: None,
            },
            navigation: {
                let nav_source = if state.manual_override {
                    Some(NavigationCapabilityEnum::ManualNavigation)
                } else if state.has_waypoints {
                    Some(NavigationCapabilityEnum::MissionPlanNavigation)
                } else {
                    None
                };

                nav_source.map(|ns| NavigationSourceDetailsType {
                    activity_id: None,
                    navigation_source: Some(ns),
                })
            },
        },
    };
    NavigationReportWrapper {
        navigation_report: mt,
    }
}

/// Configure the LA-CAL OWP connection manager.
#[derive(Debug, Clone)]
pub struct OwpPublisherConfig {
    ws_url: String,
    service_id: String,
    system_uuid: uuid::Uuid,
    subsystem_uuid: uuid::Uuid,
    mission_uuid: uuid::Uuid,
    classification: ClassificationEnum,
    owner_producer: OwnerProducerEnum,
    position_hz: f64,
    prd_hz: f64,
    initial_retry_delay: Duration,
    max_retry_delay: Duration,
}

impl OwpPublisherConfig {
    /// Create connection settings for an OWP WebSocket URL.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        ws_url: impl Into<String>,
        service_id: impl Into<String>,
        system_uuid: uuid::Uuid,
        subsystem_uuid: uuid::Uuid,
        mission_uuid: uuid::Uuid,
        classification: ClassificationEnum,
        owner_producer: OwnerProducerEnum,
        position_hz: f64,
        prd_hz: f64,
    ) -> Self {
        Self {
            ws_url: ws_url.into(),
            service_id: service_id.into(),
            system_uuid,
            subsystem_uuid,
            mission_uuid,
            classification,
            owner_producer,
            position_hz,
            prd_hz,
            initial_retry_delay: DEFAULT_INITIAL_RETRY_DELAY,
            max_retry_delay: DEFAULT_MAX_RETRY_DELAY,
        }
    }

    /// Create connection settings from LA-CAL configuration.
    pub fn from_la_cal(config: &LaCalConfig) -> Result<Self> {
        let (system_uuid, subsystem_uuid, mission_uuid) = config.resolve_uuids()?;
        Ok(Self::new(
            config.ws_url.clone(),
            config.service_id.clone(),
            system_uuid,
            subsystem_uuid,
            mission_uuid,
            config.classification.clone(),
            config.owner_producer.clone(),
            config.position_hz,
            config.prd_hz,
        ))
    }

    /// Override retry backoff timing.
    #[must_use]
    pub fn with_retry_backoff(mut self, initial: Duration, max: Duration) -> Self {
        self.initial_retry_delay = initial;
        self.max_retry_delay = max.max(initial);
        self
    }

    /// Return the configured OWP WebSocket URL.
    pub fn ws_url(&self) -> &str {
        &self.ws_url
    }

    /// Return the service ID to use when connecting to LA-CAL.
    pub fn service_id(&self) -> &str {
        &self.service_id
    }

    /// Return the system UUID.
    pub fn system_uuid(&self) -> uuid::Uuid {
        self.system_uuid
    }

    /// Return the subsystem UUID.
    pub fn subsystem_uuid(&self) -> uuid::Uuid {
        self.subsystem_uuid
    }

    /// Return the mission UUID.
    pub fn mission_uuid(&self) -> uuid::Uuid {
        self.mission_uuid
    }

    /// Return the classification.
    pub fn classification(&self) -> &ClassificationEnum {
        &self.classification
    }

    /// Return the owner/producer.
    pub fn owner_producer(&self) -> &OwnerProducerEnum {
        &self.owner_producer
    }

    /// Return the publication rate for `PositionReport` in Hz.
    pub fn position_hz_rate(&self) -> f64 {
        self.position_hz
    }

    /// Return the publication rate for `SystemStatus` in Hz.
    pub fn prd_hz_rate(&self) -> f64 {
        self.prd_hz
    }
}

/// Send entity-state updates to the background OWP manager.
pub struct OwpPublisherHandle {
    state_tx: watch::Sender<Option<EntityState>>,
    shutdown_tx: watch::Sender<bool>,
    join_handle: Mutex<Option<JoinHandle<()>>>,
}

impl OwpPublisherHandle {
    /// Spawn the background OWP connection manager.
    pub fn spawn(config: &OwpPublisherConfig, startup_complete: Arc<AtomicBool>) -> Result<Self> {
        let (state_tx, state_rx) = watch::channel(None);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let thread_config = config.clone();

        let join_handle = thread::Builder::new()
            .name("owp-la-cal".into())
            .spawn(move || run_owp_thread(thread_config, state_rx, shutdown_rx, startup_complete))
            .context("spawn OWP connection manager thread")?;

        info!(
            ws_url = %config.ws_url(),
            mission_id = %config.mission_uuid(),
            "OWP connection manager started"
        );

        Ok(Self {
            state_tx,
            shutdown_tx,
            join_handle: Mutex::new(Some(join_handle)),
        })
    }

    /// Publish the latest entity state to the OWP background manager.
    pub fn update_entity_state(&self, state: EntityState) {
        self.state_tx.send_replace(Some(state));
    }

    /// Subscribe to the latest entity state observed by the OWP manager.
    pub fn subscribe_state(&self) -> watch::Receiver<Option<EntityState>> {
        self.state_tx.subscribe()
    }
}

impl Drop for OwpPublisherHandle {
    fn drop(&mut self) {
        let _ = self.shutdown_tx.send(true);

        let Ok(mut join_handle) = self.join_handle.lock() else {
            error!("OWP connection manager join handle lock poisoned");
            return;
        };

        if let Some(join_handle) = join_handle.take()
            && join_handle.thread().id() != thread::current().id()
            && join_handle.join().is_err()
        {
            error!("OWP connection manager thread panicked during shutdown");
        }
    }
}

fn run_owp_thread(
    config: OwpPublisherConfig,
    state_rx: watch::Receiver<Option<EntityState>>,
    shutdown_rx: watch::Receiver<bool>,
    startup_complete: Arc<AtomicBool>,
) {
    let runtime = match Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            error!(%error, "OWP Tokio runtime construction failed");
            return;
        }
    };

    runtime.block_on(run_connection_loop(
        config,
        state_rx,
        shutdown_rx,
        startup_complete,
    ));
}

async fn run_connection_loop(
    config: OwpPublisherConfig,
    mut state_rx: watch::Receiver<Option<EntityState>>,
    mut shutdown_rx: watch::Receiver<bool>,
    startup_complete: Arc<AtomicBool>,
) {
    let mut backoff = LinearBackoff::new(config.initial_retry_delay, config.max_retry_delay);

    while !*shutdown_rx.borrow() {
        let options = InitOptions {
            verbose: false,
            ..Default::default()
        };

        let connect_future =
            CalClient::connect_with_options(config.ws_url(), config.service_id(), options);
        tokio::pin!(connect_future);

        let connect_result = tokio::select! {
            res = &mut connect_future => Some(res),
            res = shutdown_rx.changed() => {
                if res.is_err() || *shutdown_rx.borrow() {
                    break;
                }
                None
            }
        };

        let Some(result) = connect_result else {
            continue;
        };

        match result {
            Ok(client) => {
                info!(
                    ws_url = %config.ws_url(),
                    "OWP WebSocket connected"
                );
                backoff.reset();

                match drain_connection(
                    client,
                    &config,
                    &mut state_rx,
                    &mut shutdown_rx,
                    Arc::clone(&startup_complete),
                )
                .await
                {
                    ConnectionEnd::Shutdown => break,
                    ConnectionEnd::ClientError(error) => {
                        warn!(ws_url = %config.ws_url(), %error, "OWP connection failed");
                    }
                    ConnectionEnd::StateChannelClosed => {
                        debug!(ws_url = %config.ws_url(), "OWP state channel closed");
                    }
                }
            }
            Err(error) => {
                warn!(ws_url = %config.ws_url(), %error, "OWP WebSocket connect failed");
            }
        }

        if wait_for_retry(backoff.next_delay(), &mut shutdown_rx).await {
            break;
        }
    }

    info!(ws_url = %config.ws_url(), "OWP connection manager stopped");
}

async fn drain_connection(
    mut client: CalClient,
    config: &OwpPublisherConfig,
    state_rx: &mut watch::Receiver<Option<EntityState>>,
    shutdown_rx: &mut watch::Receiver<bool>,
    startup_complete: Arc<AtomicBool>,
) -> ConnectionEnd {
    let mut pos_interval =
        tokio::time::interval(Duration::from_secs_f64(1.0 / config.position_hz_rate()));
    pos_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let mut sys_interval =
        tokio::time::interval(Duration::from_secs_f64(1.0 / config.prd_hz_rate()));
    sys_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    while !*shutdown_rx.borrow() {
        tokio::select! {
            res = shutdown_rx.changed() => {
                if res.is_err() || *shutdown_rx.borrow() {
                    break;
                }
            }

            _ = pos_interval.tick() => {
                let state_opt = state_rx.borrow().clone();
                if let Some(state) = state_opt {
                    let timestamp = time::OffsetDateTime::now_utc()
                        .format(&time::format_description::well_known::Iso8601::DEFAULT)
                        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string());

                    let pr = build_position_report(&state, config, &timestamp);
                    if let Err(e) = client.publish("mission.position-report", &pr).await {
                        return ConnectionEnd::ClientError(e);
                    }
                }
            }

            _ = sys_interval.tick() => {
                let state_opt = state_rx.borrow().clone();
                if let Some(state) = state_opt {
                    let timestamp = time::OffsetDateTime::now_utc()
                        .format(&time::format_description::well_known::Iso8601::DEFAULT)
                        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string());

                    let is_ready = startup_complete.load(Ordering::SeqCst);
                    let ss = build_system_status(config, &timestamp, is_ready);
                    tracing::debug!(timestamp = %timestamp, "publishing mission.system-status to sleet");
                    if let Err(e) = client.publish("mission.system-status", &ss).await {
                        return ConnectionEnd::ClientError(e);
                    }

                    if !state.waypoints.is_empty() {
                        let rp = build_route_plan(&state, config, &timestamp);
                        tracing::debug!(timestamp = %timestamp, "publishing mission.route-plan to sleet");
                        if let Err(e) = client.publish("mission.route-plan", &rp).await {
                            return ConnectionEnd::ClientError(e);
                        }
                    }

                    let nr = build_navigation_report(&state, config, &timestamp);
                    tracing::debug!(timestamp = %timestamp, "publishing mission.navigation-report to sleet");
                    if let Err(e) = client.publish("mission.navigation-report", &nr).await {
                        return ConnectionEnd::ClientError(e);
                    }
                }
            }

            res = state_rx.changed() => {
                if res.is_err() {
                    return ConnectionEnd::StateChannelClosed;
                }
            }

            msg = client.recv() => {
                match msg {
                    Ok(_) => {} // Ignore unexpected incoming messages on a publisher-only client
                    Err(e) => return ConnectionEnd::ClientError(e),
                }
            }
        }
    }

    ConnectionEnd::Shutdown
}

async fn wait_for_retry(delay: Duration, shutdown_rx: &mut watch::Receiver<bool>) -> bool {
    if *shutdown_rx.borrow() {
        return true;
    }

    let sleep = tokio::time::sleep(delay);
    tokio::pin!(sleep);

    tokio::select! {
        () = &mut sleep => false,
        res = shutdown_rx.changed() => {
            res.is_err() || *shutdown_rx.borrow()
        }
    }
}

enum ConnectionEnd {
    Shutdown,
    ClientError(sleet_client::ClientError),
    StateChannelClosed,
}

#[derive(Debug, Clone)]
struct LinearBackoff {
    initial: Duration,
    max: Duration,
    current: Duration,
}

impl LinearBackoff {
    fn new(initial: Duration, max: Duration) -> Self {
        let max = max.max(initial);
        Self {
            initial,
            max,
            current: initial,
        }
    }

    fn next_delay(&mut self) -> Duration {
        let delay = self.current;
        self.current = (self.current + self.initial).min(self.max);
        delay
    }

    fn reset(&mut self) {
        self.current = self.initial;
    }
}

#[cfg(test)]
#[allow(clippy::result_large_err)]
mod tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use tokio::net::TcpListener;
    use tokio_tungstenite::accept_hdr_async;
    use tokio_tungstenite::tungstenite::Message;
    use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};

    #[test]
    fn system_status_logic() {
        let config = OwpPublisherConfig::new(
            "ws://localhost",
            "supercell",
            uuid::Uuid::nil(),
            uuid::Uuid::nil(),
            uuid::Uuid::nil(),
            ClassificationEnum::U,
            OwnerProducerEnum::Usa,
            10.0,
            1.0,
        );

        // When not ready, system state is Inactive
        let ss_not_ready = build_system_status(&config, "2025-01-01T00:00:00Z", false);
        assert_eq!(
            ss_not_ready.system_status.message_data.system_state,
            SystemStateEnum::Inactive
        );

        // When ready, system state is Operational
        let ss_ready = build_system_status(&config, "2025-01-01T00:00:00Z", true);
        assert_eq!(
            ss_ready.system_status.message_data.system_state,
            SystemStateEnum::Operational
        );
    }

    #[test]
    fn navigation_report_logic() {
        let config = OwpPublisherConfig::new(
            "ws://localhost",
            "supercell",
            uuid::Uuid::nil(),
            uuid::Uuid::nil(),
            uuid::Uuid::nil(),
            ClassificationEnum::U,
            OwnerProducerEnum::Usa,
            10.0,
            1.0,
        );

        // 1. Manual override takes precedence
        let mut state = EntityState {
            manual_override: true,
            has_waypoints: true,
            ..EntityState::default()
        };
        let nr = build_navigation_report(&state, &config, "2025-01-01T00:00:00Z");
        let nav = nr
            .navigation_report
            .message_data
            .navigation
            .expect("should have navigation block");
        assert_eq!(
            nav.navigation_source,
            Some(NavigationCapabilityEnum::ManualNavigation)
        );

        // 2. Mission plan navigation (has waypoints, no manual override)
        state.manual_override = false;
        state.has_waypoints = true;
        let nr = build_navigation_report(&state, &config, "2025-01-01T00:00:00Z");
        let nav = nr
            .navigation_report
            .message_data
            .navigation
            .expect("should have navigation block");
        assert_eq!(
            nav.navigation_source,
            Some(NavigationCapabilityEnum::MissionPlanNavigation)
        );

        // 3. No waypoints, no manual override
        state.manual_override = false;
        state.has_waypoints = false;
        let nr = build_navigation_report(&state, &config, "2025-01-01T00:00:00Z");
        assert!(
            nr.navigation_report.message_data.navigation.is_none(),
            "should omit navigation block when no waypoints and no manual override"
        );
    }

    async fn spawn_mock_server() -> std::net::SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = accept_hdr_async(stream, |_: &Request, mut response: Response| {
                response
                    .headers_mut()
                    .insert("Sec-WebSocket-Protocol", "owp".parse().unwrap());
                Ok(response)
            })
            .await
            .unwrap();

            if let Some(Ok(Message::Text(text))) = ws.next().await {
                assert!(text.starts_with("INIT "));
                let info = "INFO {\"version\":\"1.0\",\"server_id\":\"mock\",\"uuids\":{\"system\":\"s\",\"service\":\"v\"},\"system_label\":\"mock\"}";
                ws.send(Message::Text(info.into())).await.unwrap();
            }

            while ws.next().await.is_some() {}
        });

        addr
    }

    fn dummy_uuid() -> uuid::Uuid {
        uuid::Uuid::parse_str("00000000-0000-1000-8000-000000000001").unwrap()
    }

    #[tokio::test]
    async fn drain_connection_exits_on_shutdown_channel_drop() {
        let (_state_tx, mut state_rx) = watch::channel(None);
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);

        let addr = spawn_mock_server().await;
        let client = CalClient::connect_with_options(
            &format!("ws://{addr}"),
            "supercell",
            InitOptions {
                verbose: false,
                ..Default::default()
            },
        )
        .await
        .expect("connect should succeed");

        let config = OwpPublisherConfig::new(
            "ws://dummy",
            "supercell",
            dummy_uuid(),
            dummy_uuid(),
            dummy_uuid(),
            ClassificationEnum::U,
            OwnerProducerEnum::Usa,
            10.0,
            2.0,
        );

        let drain_task = tokio::spawn(async move {
            drain_connection(
                client,
                &config,
                &mut state_rx,
                &mut shutdown_rx,
                Arc::new(AtomicBool::new(false)),
            )
            .await
        });

        tokio::task::yield_now().await;
        drop(shutdown_tx);

        let result = tokio::time::timeout(Duration::from_millis(500), drain_task)
            .await
            .expect("drain_connection hung on shutdown drop");

        assert!(
            matches!(result.unwrap(), ConnectionEnd::Shutdown),
            "expected Shutdown end state"
        );
    }

    #[tokio::test]
    async fn drain_connection_exits_on_shutdown() {
        let (state_tx, mut state_rx) = watch::channel(None);
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);

        let addr = spawn_mock_server().await;
        let client = CalClient::connect_with_options(
            &format!("ws://{addr}"),
            "supercell",
            InitOptions {
                verbose: false,
                ..Default::default()
            },
        )
        .await
        .expect("connect should succeed");

        let config = OwpPublisherConfig::new(
            "ws://dummy",
            "supercell",
            dummy_uuid(),
            dummy_uuid(),
            dummy_uuid(),
            ClassificationEnum::U,
            OwnerProducerEnum::Usa,
            10.0,
            2.0,
        );

        let drain_task = tokio::spawn(async move {
            drain_connection(
                client,
                &config,
                &mut state_rx,
                &mut shutdown_rx,
                Arc::new(AtomicBool::new(false)),
            )
            .await
        });

        tokio::task::yield_now().await;
        state_tx.send_replace(Some(EntityState::default()));
        tokio::task::yield_now().await;

        shutdown_tx.send_replace(true);

        let result = tokio::time::timeout(Duration::from_millis(500), drain_task)
            .await
            .expect("drain_connection hung on shutdown");

        assert!(
            matches!(result.unwrap(), ConnectionEnd::Shutdown),
            "expected Shutdown end state"
        );
    }

    #[tokio::test]
    async fn wait_for_retry_exits_on_shutdown_channel_drop() {
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);

        let delay = Duration::from_secs(10);
        let wait_task = tokio::spawn(async move { wait_for_retry(delay, &mut shutdown_rx).await });

        // Yield to allow wait_task to enter the select sleep
        tokio::task::yield_now().await;

        // Trigger shutdown drop
        drop(shutdown_tx);

        let result = tokio::time::timeout(Duration::from_millis(500), wait_task)
            .await
            .expect("wait_for_retry hung on shutdown drop");

        assert!(
            result.unwrap(),
            "expected wait_for_retry to return true (break) on channel drop"
        );
    }

    #[test]
    fn linear_backoff_increases_linearly_and_caps() {
        let mut backoff = LinearBackoff::new(Duration::from_secs(1), Duration::from_secs(3));

        assert_eq!(backoff.next_delay(), Duration::from_secs(1));
        assert_eq!(backoff.next_delay(), Duration::from_secs(2));
        assert_eq!(backoff.next_delay(), Duration::from_secs(3));
        assert_eq!(backoff.next_delay(), Duration::from_secs(3));
    }

    #[test]
    fn linear_backoff_reset_restores_initial_delay() {
        let mut backoff = LinearBackoff::new(Duration::from_secs(1), Duration::from_secs(3));

        assert_eq!(backoff.next_delay(), Duration::from_secs(1));
        assert_eq!(backoff.next_delay(), Duration::from_secs(2));
        backoff.reset();
        assert_eq!(backoff.next_delay(), Duration::from_secs(1));
    }
    #[test]
    fn deterministic_uuid_generation_handles_large_counts() {
        let state = crate::entity::EntityState {
            waypoints: (0..300)
                .map(|_| crate::config::Waypoint {
                    latitude_deg: 0.0,
                    longitude_deg: 0.0,
                    altitude_m: 0.0,
                })
                .collect(),
            ..crate::entity::EntityState::default()
        };
        let config = OwpPublisherConfig::new(
            "ws://localhost",
            "supercell",
            uuid::Uuid::nil(),
            uuid::Uuid::nil(),
            uuid::Uuid::nil(),
            ClassificationEnum::U,
            OwnerProducerEnum::Usa,
            10.0,
            2.0,
        );
        let rp = build_route_plan(&state, &config, "2026-05-07T00:00:00Z");

        let path = &rp.route_plan.message_data.plan.route.path[0];
        let uuids: std::collections::HashSet<_> = path
            .path_segment
            .iter()
            .map(|seg| &seg.path_segment_id.uuid)
            .collect();
        assert_eq!(uuids.len(), 300, "Should generate exactly 300 unique UUIDs");
    }
}

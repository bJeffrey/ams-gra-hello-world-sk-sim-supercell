//! Manage the OMS LA-CAL OWP WebSocket connection.
//!
//! The manager owns a background Tokio runtime so WebSocket I/O cannot block the
//! synchronous simulation loop. It keeps the latest entity state available for
//! publishing logic while connection establishment and reconnect behavior run in
//! the background.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tokio::runtime::Builder;
use tokio::sync::{mpsc, watch};
use tracing::{debug, error, info, warn};

use sleet_client::{CalClient, InitOptions};
use sleet_types::uci::v2_5::{
    AltitudeReferenceEnum, ClassificationEnum, DetailedKinematicsErrorType, DetailedKinematicsType,
    EndPointType, EnduranceType, HeaderType, LineProjectionEnum, MessageModeEnum,
    NavigationCapabilityEnum, NavigationReportMdt, NavigationReportMt, NavigationSolutionStateEnum,
    NavigationSourceDetailsType, OwnerProducerChoiceType, OwnerProducerEnum, PathIdType,
    PathSegmentSourceEnum, PathSegmentType, PathTypeEnum, PlanApplicabilityType, Point2DType,
    Point4DType, PointChoice4DType, PositionPositionCovarianceType, PositionReportDataType,
    PositionReportDetailedMdt, PositionReportDetailedMt, PositionSourceIdChoiceType,
    PositionVelocityCovarianceType, RoutePathType, RoutePlanIdType, RoutePlanMdt, RoutePlanMt,
    RoutePlanPartsType, RoutePlanType, RouteType, SecurityInformationType, SegmentIdType,
    SubsystemIdType, SystemContingencyLevelEnum, SystemIdType, SystemSourceEnum, SystemStateEnum,
    SystemStatusMdt, SystemStatusMt, Velocity3DType, VelocityVelocityCovarianceType,
    WayPointPointChoiceType, WayPointType, WaypointTypeEnum,
};

use crate::config::LaCalConfig;
use crate::entity::{EntityState, TimedEntityState};

const DEFAULT_INITIAL_RETRY_DELAY: Duration = Duration::from_secs(1);
const DEFAULT_MAX_RETRY_DELAY: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct PublicationDue {
    position: bool,
    periodic: bool,
    coalesced_position: u64,
    coalesced_periodic: u64,
}

#[derive(Debug)]
struct PublicationSchedule {
    next_position_time: Option<time::OffsetDateTime>,
    next_periodic_time: Option<time::OffsetDateTime>,
    last_scenario_time: Option<time::OffsetDateTime>,
    position_period: Duration,
    periodic_period: Duration,
}

impl PublicationSchedule {
    fn new(position_hz: f64, periodic_hz: f64) -> Self {
        Self {
            next_position_time: None,
            next_periodic_time: None,
            last_scenario_time: None,
            position_period: Duration::from_secs_f64(1.0 / position_hz),
            periodic_period: Duration::from_secs_f64(1.0 / periodic_hz),
        }
    }

    fn update(&mut self, current: time::OffsetDateTime) -> PublicationDue {
        if self.last_scenario_time.is_some_and(|last| current < last) {
            self.next_position_time = None;
            self.next_periodic_time = None;
        }
        self.last_scenario_time = Some(current);

        let position_deadline = self.next_position_time.get_or_insert(current);
        let periodic_deadline = self.next_periodic_time.get_or_insert(current);
        let position_count =
            advance_deadline_past(position_deadline, self.position_period, current);
        let periodic_count =
            advance_deadline_past(periodic_deadline, self.periodic_period, current);

        PublicationDue {
            position: position_count > 0,
            periodic: periodic_count > 0,
            coalesced_position: position_count.saturating_sub(1),
            coalesced_periodic: periodic_count.saturating_sub(1),
        }
    }
}

fn advance_deadline_past(
    deadline: &mut time::OffsetDateTime,
    period: Duration,
    current: time::OffsetDateTime,
) -> u64 {
    let mut count = 0_u64;
    while *deadline <= current {
        *deadline += period;
        count = count.saturating_add(1);
    }
    count
}

#[derive(Debug)]
struct WallRateLimiter {
    minimum_interval: Option<Duration>,
    last_publish: Option<Instant>,
}

impl WallRateLimiter {
    fn new(max_wall_publish_hz: Option<f64>) -> Self {
        Self {
            minimum_interval: max_wall_publish_hz.map(|rate| Duration::from_secs_f64(1.0 / rate)),
            last_publish: None,
        }
    }

    fn allow(&mut self, now: Instant) -> bool {
        let Some(minimum_interval) = self.minimum_interval else {
            return true;
        };
        if self
            .last_publish
            .is_some_and(|last| now.duration_since(last) < minimum_interval)
        {
            return false;
        }
        self.last_publish = Some(now);
        true
    }
}

#[derive(serde::Serialize)]
struct PositionReportDetailedWrapper {
    #[serde(rename = "PositionReportDetailed")]
    position_report_detailed: PositionReportDetailedMt,
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

fn platform_system_uuid(state: &EntityState, config: &OwpPublisherConfig) -> uuid::Uuid {
    if state.entity_id == config.ownship_entity_id() {
        return config.system_uuid();
    }
    uuid::Uuid::new_v5(
        &config.system_uuid(),
        format!(
            "dis-platform:{}:{}:{}",
            state.site_id, state.application_id, state.entity_id
        )
        .as_bytes(),
    )
}

fn platform_egi_uuid(state: &EntityState, config: &OwpPublisherConfig) -> uuid::Uuid {
    if state.entity_id == config.ownship_entity_id() {
        return config.subsystem_uuid();
    }
    uuid::Uuid::new_v5(&platform_system_uuid(state, config), b"egi")
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

fn build_position_report_detailed(
    state: &EntityState,
    config: &OwpPublisherConfig,
    timestamp: &str,
) -> PositionReportDetailedWrapper {
    let timing_variance = config.navigation_timing_error_seconds().powi(2);
    let position_position_covariance = PositionPositionCovarianceType {
        pn_pn: state.velocity_north_mps.powi(2) * timing_variance,
        pn_pe: 0.0,
        pn_pd: Some(0.0),
        pe_pe: state.velocity_east_mps.powi(2) * timing_variance,
        pe_pd: Some(0.0),
        pd_pd: Some(state.velocity_down_mps.powi(2) * timing_variance),
    };
    let position_velocity_covariance = PositionVelocityCovarianceType {
        pn_vn: state.velocity_north_mps * state.acceleration_north_mps2 * timing_variance,
        pn_ve: 0.0,
        pn_vd: Some(0.0),
        pe_vn: 0.0,
        pe_ve: state.velocity_east_mps * state.acceleration_east_mps2 * timing_variance,
        pe_vd: Some(0.0),
        pd_vn: Some(0.0),
        pd_ve: Some(0.0),
        pd_vd: Some(state.velocity_down_mps * state.acceleration_down_mps2 * timing_variance),
    };
    let velocity_velocity_covariance = VelocityVelocityCovarianceType {
        vn_vn: state.acceleration_north_mps2.powi(2) * timing_variance,
        vn_ve: 0.0,
        vn_vd: Some(0.0),
        ve_ve: state.acceleration_east_mps2.powi(2) * timing_variance,
        ve_vd: Some(0.0),
        vd_vd: Some(state.acceleration_down_mps2.powi(2) * timing_variance),
    };

    let mt = PositionReportDetailedMt {
        security_information: build_security_info(config),
        message_header: build_header(config, timestamp),
        message_data: PositionReportDetailedMdt {
            position_report_data: vec![PositionReportDataType {
                position_source: PositionSourceIdChoiceType::SubsystemId {
                    subsystem_id: SubsystemIdType {
                        uuid: platform_egi_uuid(state, config).to_string(),
                        descriptive_label: Some("EGI".to_string()),
                    },
                },
                component_id: None,
                navigation_solution_state: NavigationSolutionStateEnum::Blended,
                figure_of_merit: None,
                kinematics: DetailedKinematicsType {
                    position: PointChoice4DType::AbsolutePoint {
                        absolute_point: Point4DType {
                            latitude: state.latitude_deg.to_radians(),
                            longitude: state.longitude_deg.to_radians(),
                            altitude: state.altitude_m,
                            altitude_reference: Some(AltitudeReferenceEnum::WgsHae),
                            timestamp: timestamp.to_string(),
                            depth_category: None,
                            hae_adjustment: None,
                        },
                    },
                    velocity: Velocity3DType {
                        north_speed: state.velocity_north_mps,
                        east_speed: state.velocity_east_mps,
                        down_speed: state.velocity_down_mps,
                        timestamp: Some(timestamp.to_string()),
                    },
                    air_data: None,
                    acceleration: None,
                    orientation: None,
                    wander_angle: None,
                    magnetic_heading: None,
                    orientation_rate: None,
                    orientation_acceleration: None,
                },
                kinematics_error: DetailedKinematicsErrorType {
                    position_position_covariance,
                    position_velocity_covariance,
                    velocity_velocity_covariance,
                    orientation_orientation_covariance: None,
                    position_orientation_covariance: None,
                    velocity_orientation_covariance: None,
                },
                solution_corrections: None,
            }],
            simulation_target_number: Some(
                ((state.site_id as i64) << 32)
                    | ((state.application_id as i64) << 16)
                    | (state.entity_id as i64),
            ),
        },
    };
    PositionReportDetailedWrapper {
        position_report_detailed: mt,
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
    navigation_timing_error_seconds: f64,
    ownship_entity_id: u16,
    max_wall_publish_hz: Option<f64>,
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
            navigation_timing_error_seconds: 0.01,
            ownship_entity_id: 0,
            max_wall_publish_hz: None,
            initial_retry_delay: DEFAULT_INITIAL_RETRY_DELAY,
            max_retry_delay: DEFAULT_MAX_RETRY_DELAY,
        }
    }

    /// Create connection settings from LA-CAL configuration.
    pub fn from_la_cal(config: &LaCalConfig) -> Result<Self> {
        let (system_uuid, subsystem_uuid, mission_uuid) = config.resolve_uuids()?;
        let mut resolved = Self::new(
            config.ws_url.clone(),
            config.service_id.clone(),
            system_uuid,
            subsystem_uuid,
            mission_uuid,
            config.classification.clone(),
            config.owner_producer.clone(),
            config.position_hz,
            config.prd_hz,
        );
        resolved.navigation_timing_error_seconds = config.navigation_timing_error_seconds;
        Ok(resolved)
    }

    /// Set an optional wall-monotonic limit for OWP publication batches.
    #[must_use]
    pub fn with_max_wall_publish_hz(mut self, max_wall_publish_hz: Option<f64>) -> Self {
        self.max_wall_publish_hz = max_wall_publish_hz;
        self
    }

    /// Set the DIS entity ID that retains the configured ownship UCI identities.
    #[must_use]
    pub fn with_ownship_entity_id(mut self, ownship_entity_id: u16) -> Self {
        self.ownship_entity_id = ownship_entity_id;
        self
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

    /// Return the configured one-sigma EGI timing uncertainty in seconds.
    pub fn navigation_timing_error_seconds(&self) -> f64 {
        self.navigation_timing_error_seconds
    }

    /// Return the configured ownship DIS entity ID.
    pub fn ownship_entity_id(&self) -> u16 {
        self.ownship_entity_id
    }

    /// Return the optional wall-monotonic publication-batch rate limit.
    pub fn max_wall_publish_hz(&self) -> Option<f64> {
        self.max_wall_publish_hz
    }
}

/// Send entity-state updates to the background OWP manager.
pub struct OwpPublisherHandle {
    state_tx: watch::Sender<Option<TimedEntityState>>,
    state_event_tx: mpsc::Sender<TimedEntityState>,
    shutdown_tx: watch::Sender<bool>,
    join_handle: Mutex<Option<JoinHandle<()>>>,
}

impl OwpPublisherHandle {
    /// Spawn the background OWP connection manager.
    pub fn spawn(config: &OwpPublisherConfig, startup_complete: Arc<AtomicBool>) -> Result<Self> {
        let (state_tx, _state_rx) = watch::channel(None);
        let (state_event_tx, state_event_rx) = mpsc::channel(1024);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let thread_config = config.clone();

        let join_handle = thread::Builder::new()
            .name("owp-la-cal".into())
            .spawn(move || {
                run_owp_thread(thread_config, state_event_rx, shutdown_rx, startup_complete);
            })
            .context("spawn OWP connection manager thread")?;

        info!(
            ws_url = %config.ws_url(),
            mission_id = %config.mission_uuid(),
            "OWP connection manager started"
        );

        Ok(Self {
            state_tx,
            state_event_tx,
            shutdown_tx,
            join_handle: Mutex::new(Some(join_handle)),
        })
    }

    /// Publish the latest entity state to the OWP background manager.
    pub fn update_entity_state(&self, state: EntityState) {
        self.update_timed_entity_state(TimedEntityState {
            state,
            scenario_time: time::OffsetDateTime::now_utc(),
            tick: 0,
        });
    }

    /// Publish entity state stamped with authoritative scenario time.
    pub fn update_timed_entity_state(&self, state: TimedEntityState) {
        self.state_tx.send_replace(Some(state.clone()));
        if let Err(error) = self.state_event_tx.try_send(state) {
            warn!(%error, "coalesced OWP state update because event queue is unavailable");
        }
    }

    /// Subscribe to the latest entity state observed by the OWP manager.
    pub fn subscribe_state(&self) -> watch::Receiver<Option<TimedEntityState>> {
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
    state_event_rx: mpsc::Receiver<TimedEntityState>,
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
        state_event_rx,
        shutdown_rx,
        startup_complete,
    ));
}

async fn run_connection_loop(
    config: OwpPublisherConfig,
    mut state_event_rx: mpsc::Receiver<TimedEntityState>,
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
                    &mut state_event_rx,
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
    state_event_rx: &mut mpsc::Receiver<TimedEntityState>,
    shutdown_rx: &mut watch::Receiver<bool>,
    startup_complete: Arc<AtomicBool>,
) -> ConnectionEnd {
    let mut schedules: HashMap<(u16, u16, u16), PublicationSchedule> = HashMap::new();
    let mut wall_rate_limiters: HashMap<(u16, u16, u16), WallRateLimiter> = HashMap::new();

    while !*shutdown_rx.borrow() {
        tokio::select! {
            res = shutdown_rx.changed() => {
                if res.is_err() || *shutdown_rx.borrow() {
                    break;
                }
            }

            state_opt = state_event_rx.recv() => {
                let Some(timed_state) = state_opt else {
                    return ConnectionEnd::StateChannelClosed;
                };
                    let key = (
                        timed_state.state.site_id,
                        timed_state.state.application_id,
                        timed_state.state.entity_id,
                    );
                    let due = schedules
                        .entry(key)
                        .or_insert_with(|| PublicationSchedule::new(config.position_hz_rate(), config.prd_hz_rate()))
                        .update(timed_state.scenario_time);
                    let wall_rate_limiter = wall_rate_limiters
                        .entry(key)
                        .or_insert_with(|| WallRateLimiter::new(config.max_wall_publish_hz()));
                    if (due.position || due.periodic) && !wall_rate_limiter.allow(Instant::now()) {
                        tracing::debug!(
                            tick = timed_state.tick,
                            scenario_time = %timed_state.scenario_time,
                            "coalesced OWP publication at wall-rate limit"
                        );
                        continue;
                    }
                    let timestamp = timed_state.scenario_time
                        .format(&time::format_description::well_known::Iso8601::DEFAULT)
                        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string());
                    let state = &timed_state.state;

                    if due.position {
                        let pr = build_position_report_detailed(state, config, &timestamp);
                        if let Err(e) = client.publish("mission.position-report-detailed", &pr).await {
                            return ConnectionEnd::ClientError(e);
                        }
                    }

                    if due.periodic && timed_state.state.entity_id == config.ownship_entity_id() {
                        let is_ready = startup_complete.load(Ordering::SeqCst);
                        let ss = build_system_status(config, &timestamp, is_ready);
                        tracing::debug!(timestamp = %timestamp, "publishing mission.system-status to sleet");
                        if let Err(e) = client.publish("mission.system-status", &ss).await {
                            return ConnectionEnd::ClientError(e);
                        }

                        if !state.waypoints.is_empty() {
                            let rp = build_route_plan(state, config, &timestamp);
                            tracing::debug!(timestamp = %timestamp, "publishing mission.route-plan to sleet");
                            if let Err(e) = client.publish("mission.route-plan", &rp).await {
                                return ConnectionEnd::ClientError(e);
                            }
                        }

                        let nr = build_navigation_report(state, config, &timestamp);
                        tracing::debug!(timestamp = %timestamp, "publishing mission.navigation-report to sleet");
                        if let Err(e) = client.publish("mission.navigation-report", &nr).await {
                            return ConnectionEnd::ClientError(e);
                        }
                    }

                    if due.coalesced_position > 0 || due.coalesced_periodic > 0 {
                        tracing::debug!(
                            tick = timed_state.tick,
                            coalesced_position = due.coalesced_position,
                            coalesced_periodic = due.coalesced_periodic,
                            "coalesced missed scenario-time publication deadlines"
                        );
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
    fn publication_schedule_is_due_on_first_state_and_exact_deadlines() {
        let start = time::OffsetDateTime::UNIX_EPOCH;
        let mut schedule = PublicationSchedule::new(10.0, 2.0);

        assert_eq!(
            schedule.update(start),
            PublicationDue {
                position: true,
                periodic: true,
                ..PublicationDue::default()
            }
        );
        assert_eq!(
            schedule.update(start + Duration::from_millis(50)),
            PublicationDue::default()
        );
        assert!(schedule.update(start + Duration::from_millis(100)).position);
    }

    #[test]
    fn publication_schedule_coalesces_missed_deadlines() {
        let start = time::OffsetDateTime::UNIX_EPOCH;
        let mut schedule = PublicationSchedule::new(10.0, 2.0);
        schedule.update(start);

        let due = schedule.update(start + Duration::from_secs(1));

        assert!(due.position);
        assert!(due.periodic);
        assert_eq!(due.coalesced_position, 9);
        assert_eq!(due.coalesced_periodic, 1);
    }

    #[test]
    fn publication_schedule_rates_are_independent() {
        let start = time::OffsetDateTime::UNIX_EPOCH;
        let mut schedule = PublicationSchedule::new(10.0, 2.0);
        schedule.update(start);

        let position_only = schedule.update(start + Duration::from_millis(100));
        assert!(position_only.position);
        assert!(!position_only.periodic);

        let both = schedule.update(start + Duration::from_millis(500));
        assert!(both.position);
        assert!(both.periodic);
    }

    #[test]
    fn publication_schedule_resets_after_time_moves_backward() {
        let start = time::OffsetDateTime::UNIX_EPOCH;
        let mut schedule = PublicationSchedule::new(10.0, 2.0);
        schedule.update(start + Duration::from_secs(5));

        let due = schedule.update(start + Duration::from_secs(1));

        assert!(due.position);
        assert!(due.periodic);
        assert_eq!(due.coalesced_position, 0);
        assert_eq!(due.coalesced_periodic, 0);
    }

    #[test]
    fn wall_rate_limiter_allows_first_and_enforces_interval() {
        let start = Instant::now();
        let mut limiter = WallRateLimiter::new(Some(10.0));

        assert!(limiter.allow(start));
        assert!(!limiter.allow(start + Duration::from_millis(99)));
        assert!(limiter.allow(start + Duration::from_millis(100)));
    }

    #[test]
    fn disabled_wall_rate_limiter_never_blocks() {
        let start = Instant::now();
        let mut limiter = WallRateLimiter::new(None);

        assert!(limiter.allow(start));
        assert!(limiter.allow(start));
    }

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
    fn detailed_position_report_uses_blended_egi_and_timing_covariance() {
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
        let state = EntityState {
            velocity_north_mps: 100.0,
            velocity_east_mps: 50.0,
            velocity_down_mps: -2.0,
            acceleration_north_mps2: 2.0,
            acceleration_east_mps2: -1.0,
            acceleration_down_mps2: 0.5,
            ..EntityState::default()
        };

        let report = build_position_report_detailed(&state, &config, "2026-01-01T00:00:01Z");
        let data = &report
            .position_report_detailed
            .message_data
            .position_report_data[0];

        assert_eq!(
            data.navigation_solution_state,
            NavigationSolutionStateEnum::Blended
        );
        assert!(data.kinematics.orientation.is_none());
        assert!(data.kinematics.acceleration.is_none());
        assert_eq!(
            data.kinematics_error.position_position_covariance.pn_pn,
            1.0
        );
        assert_eq!(
            data.kinematics_error.position_position_covariance.pe_pe,
            0.25
        );
        assert_eq!(
            data.kinematics_error.velocity_velocity_covariance.vn_vn,
            0.0004
        );
        assert_eq!(
            data.kinematics_error.position_velocity_covariance.pn_vn,
            0.02
        );

        let json = serde_json::to_value(&report).expect("serialize detailed position report");
        assert_eq!(
            json["PositionReportDetailed"]["MessageHeader"]["Timestamp"],
            "2026-01-01T00:00:01Z"
        );
        assert_eq!(
            json["PositionReportDetailed"]["MessageData"]["PositionReportData"][0]["PositionSource"]
                ["SubsystemID"]["DescriptiveLabel"],
            "EGI"
        );

        let mut wingman = state;
        wingman.entity_id = 2;
        let wingman_report =
            build_position_report_detailed(&wingman, &config, "2026-01-01T00:00:01Z");
        let wingman_json = serde_json::to_value(&wingman_report).expect("serialize wingman report");
        assert_ne!(
            json["PositionReportDetailed"]["MessageData"]["PositionReportData"][0]["PositionSource"]
                ["SubsystemID"]["UUID"],
            wingman_json["PositionReportDetailed"]["MessageData"]["PositionReportData"][0]["PositionSource"]
                ["SubsystemID"]["UUID"]
        );
    }

    #[tokio::test]
    async fn live_sleet_accepts_generated_detailed_position_report_when_configured() {
        let Ok(url) = std::env::var("SUPERCELL_SLEET_E2E_URL") else {
            return;
        };
        let options = InitOptions {
            verbose: true,
            ..Default::default()
        };
        let mut subscriber = CalClient::connect_with_options(&url, "supercell", options.clone())
            .await
            .expect("connect Sleet subscriber");
        subscriber
            .subscribe(
                "prd-e2e",
                "PositionReportDetailed",
                "mission.position-report-detailed",
                None,
            )
            .await
            .expect("subscribe to detailed position reports");
        let mut publisher = CalClient::connect_with_options(&url, "supercell", options)
            .await
            .expect("connect Sleet publisher");
        let config = OwpPublisherConfig::new(
            &url,
            "supercell",
            uuid::Uuid::nil(),
            uuid::Uuid::nil(),
            uuid::Uuid::nil(),
            ClassificationEnum::U,
            OwnerProducerEnum::Usa,
            10.0,
            1.0,
        )
        .with_ownship_entity_id(1);
        let ownship = EntityState {
                entity_id: 1,
                site_id: 1,
                application_id: 1,
                force_id: 1,
                latitude_deg: 35.0,
                longitude_deg: -118.0,
                altitude_m: 3_000.0,
                velocity_north_mps: 100.0,
                acceleration_north_mps2: 2.0,
                ..EntityState::default()
        };
        let mut wingman = ownship.clone();
        wingman.entity_id = 2;
        wingman.latitude_deg += 0.01;

        // Publish separate cooperating-platform messages because this is the
        // shape used by the running SuperCell OWP manager.
        for state in [ownship, wingman] {
            let report = build_position_report_detailed(
                &state, &config, "2026-01-01T00:00:01Z");
            publisher
                .publish("mission.position-report-detailed", &report)
                .await
                .expect("Sleet should accept the generated UCI payload");
            let received = tokio::time::timeout(Duration::from_secs(2), subscriber.recv())
                .await
                .expect("timed out waiting for routed detailed report")
                .expect("receive routed detailed report");
            let value: serde_json::Value =
                serde_json::from_str(&received.payload).expect("parse routed JSON");
            let typed: PositionReportDetailedMt =
                serde_json::from_value(value["PositionReportDetailed"].clone())
                    .expect("decode routed UCI PositionReportDetailed");
            assert_eq!(typed.message_header.timestamp, "2026-01-01T00:00:01Z");
            assert_eq!(
                typed.message_data.position_report_data[0].navigation_solution_state,
                NavigationSolutionStateEnum::Blended
            );
        }
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
        let (_state_tx, mut state_rx) = mpsc::channel(8);
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
        let (state_tx, mut state_rx) = mpsc::channel(8);
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
        state_tx
            .try_send(TimedEntityState {
                state: EntityState::default(),
                scenario_time: time::OffsetDateTime::UNIX_EPOCH,
                tick: 0,
            })
            .expect("state receiver should remain open");
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

//! SuperCell executable entrypoint.
//!
//! Loads scenario configuration, initializes runtime components, and starts the simulation loop.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, anyhow};
use tracing::{debug, error, info, warn};

use supercell::config::{EntityRef, SupercellConfig};
use supercell::dis::DisPublisher;
use supercell::entity::{EntityState, EntityStatus};
use supercell::fdm::{FdmHandle, JsbsimHandle};
use supercell::flightgear::FlightGearBridge;
use supercell::owp::{OwpPublisherConfig, OwpPublisherHandle};
use supercell::sim::{RuntimeEntity, Simulation};
use supercell::telemetry::{init_metrics, init_tracing};

fn main() -> Result<()> {
    let bootstrap_guard = supercell::telemetry::bootstrap();

    let cli_action = match parse_config_arg() {
        Ok(action) => action,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };

    let config_path = match cli_action {
        CliAction::Run { config_path } => config_path,
        CliAction::HealthCheck { port } => match run_health_check(port) {
            Ok(()) => return Ok(()),
            Err(e) => {
                eprintln!("health check failed: {e}");
                std::process::exit(1);
            }
        },
        CliAction::Help => {
            println!(
                "Usage: supercell [OPTIONS]\n\
                 \n\
                 Options:\n\
                   --config PATH               Path to scenario TOML config\n\
                   --health-check [PORT]       Run a fast local health check (defaults to SUPERCELL_ADMIN_PORT env var)\n\
                   -h, --help                  Print help"
            );
            return Ok(());
        }
    };

    let toml_src = std::fs::read_to_string(&config_path)
        .with_context(|| format!("read config file: {config_path}"))?;
    let config: SupercellConfig =
        toml::from_str(&toml_src).with_context(|| format!("parse config: {config_path}"))?;

    drop(bootstrap_guard);
    init_tracing(
        &config.log_format,
        Some(&config.log_level),
        config.otlp_endpoint.as_deref(),
    );

    let prometheus_handle = init_metrics();

    if let Err(e) = config.validate_runtime_contracts() {
        error!(error = %e, "invalid scenario config contract");
        return Err(e).context("scenario contains invalid startup config");
    }
    let time_settings = config.time_settings()?;
    let simulation_hz = time_settings.simulation_hz;

    if let Some(fg) = &config.flightgear
        && let Err(e) = fg.validate_runtime_contracts()
    {
        error!(error = %e, "invalid FlightGear config contract");
        return Err(e).context("scenario contains invalid FlightGear config");
    }

    let startup_complete = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let la_cal_status = handle_la_cal_startup(&config)?;
    let owp_publisher = start_owp_publisher(&config, la_cal_status, Arc::clone(&startup_complete))?;

    info!(
        log_format = %config.log_format,
        simulation_hz,
        entity_count = config.entities.iter_all().count(),
        "config loaded"
    );

    for entity in config.entities.iter_all() {
        let base = entity.base();
        if let Err(e) = base.validate_dis_contracts() {
            error!(
                entity_id = base.entity_id,
                name = %base.name,
                force_id = base.force_id,
                error = %e,
                "invalid entity DIS contract"
            );
            return Err(e).context("scenario contains invalid DIS force_id");
        }

        debug!(
            entity_id = base.entity_id,
            name = %base.name,
            force_id = base.force_id,
            kind = match entity {
                EntityRef::Flying(f) => format!("Flying({})", f.aircraft),
                EntityRef::Fixed(_) => "Fixed".to_string(),
            },
            "entity registered"
        );
    }

    // ── Ctrl-C handler ────────────────────────────────────────────────────────
    let running = Arc::new(AtomicBool::new(true));
    let running_ctrlc = Arc::clone(&running);
    ctrlc::set_handler(move || {
        warn!("ctrl-c received, shutting down");
        running_ctrlc.store(false, Ordering::SeqCst);
    })
    .context("set ctrl-c handler")?;

    // ── Build RuntimeEntity list ──────────────────────────────────────────────
    let mut entities = build_runtime_entities(&config, running.as_ref())?;

    // ── Validate configured ownship exists ─────────────────────────────
    let target_id = config.entities.ownship.base.entity_id;
    let ownship = entities
        .iter_mut()
        .find(|e| e.state().entity_id == target_id);
    if ownship.is_none() {
        return Err(anyhow!(
            "ownship entity_id={target_id} not found in runtime entities (this should be structurally impossible)"
        ));
    }

    // ── FlightGear bridge (optional) ─────────────────────────────────────────
    if let Some(ref fg_config) = config.flightgear {
        let fg_bridge =
            FlightGearBridge::new(fg_config).context("FlightGear bridge construction failed")?;

        match ownship.unwrap() {
            RuntimeEntity::Flying { bridge, .. } => {
                *bridge = Some(fg_bridge);
                debug!(entity_id = target_id, "FlightGear bridge attached");
            }
            RuntimeEntity::Fixed { .. } => {
                return Err(anyhow!(
                    "FlightGear bridge: ownship_entity_id={target_id} is Fixed, need Flying"
                ));
            }
        }
    }

    // ── DIS publisher ─────────────────────────────────────────────────────────
    let dis = DisPublisher::new(&config.dis).map_err(|e| {
        error!(error = %e, "DIS publisher init failed");
        e
    })?;
    info!(multicast_addr = %config.dis.multicast_addr, port = config.dis.port,
          "DIS publisher bound");

    // ── Admin Server ──────────────────────────────────────────────────────────
    let last_tick_epoch_secs = Arc::new(std::sync::atomic::AtomicU64::new(0));

    if let Some(admin_addr) = config.admin_bind_addr.clone() {
        supercell::admin::spawn_admin_server(
            admin_addr,
            Arc::clone(&last_tick_epoch_secs),
            Arc::clone(&startup_complete),
            prometheus_handle,
        );
    }

    info!(total = entities.len(), simulation_hz, "starting simulation");

    let mut simulation = Simulation::new(
        entities,
        dis,
        owp_publisher,
        config.waypoint_threshold_m,
        config.entities.ownship.base.entity_id,
    );
    simulation.start_fdms()?;

    // Signal readiness before starting the run loop.
    startup_complete.store(true, std::sync::atomic::Ordering::SeqCst);

    simulation.run_with_time(
        &running,
        &time_settings,
        config.settle_secs,
        &last_tick_epoch_secs,
    )?;

    info!("supercell shutdown complete");
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LaCalStartupStatus {
    NotConfigured,
    Ready,
}

fn handle_la_cal_startup(config: &SupercellConfig) -> Result<LaCalStartupStatus> {
    let Some(la_cal) = config.la_cal_config() else {
        return Ok(LaCalStartupStatus::NotConfigured);
    };

    let (system_uuid, subsystem_uuid, mission_uuid) = la_cal.resolve_uuids()?;

    info!(
        ws_url = %la_cal.ws_url,
        system_uuid = %system_uuid,
        subsystem_uuid = %subsystem_uuid,
        mission_uuid = %mission_uuid,
        position_hz = la_cal.position_hz,
        prd_hz = la_cal.prd_hz,
        "UCI LA-CAL configuration ready"
    );

    Ok(LaCalStartupStatus::Ready)
}

fn start_owp_publisher(
    config: &SupercellConfig,
    status: LaCalStartupStatus,
    startup_complete: Arc<std::sync::atomic::AtomicBool>,
) -> Result<Option<OwpPublisherHandle>> {
    if status != LaCalStartupStatus::Ready {
        return Ok(None);
    }

    let la_cal = config
        .la_cal_config()
        .expect("ready LA-CAL status requires present LA-CAL config");
    let owp_config = OwpPublisherConfig::from_la_cal(la_cal)?;
    let handle = OwpPublisherHandle::spawn(&owp_config, startup_complete)
        .context("start OWP connection manager")?;

    Ok(Some(handle))
}

fn run_health_check(port: u16) -> Result<(), String> {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::Duration;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .map_err(|e| format!("connection failed: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|e| format!("failed to set read timeout: {e}"))?;
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .map_err(|e| format!("failed to set write timeout: {e}"))?;

    stream
        .write_all(b"GET /health HTTP/1.0\r\nHost: localhost\r\n\r\n")
        .map_err(|e| format!("write failed: {e}"))?;

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|e| format!("read failed: {e}"))?;

    if response.starts_with("HTTP/1.0 200") || response.starts_with("HTTP/1.1 200") {
        Ok(())
    } else {
        Err(format!(
            "unhealthy response: {}",
            response.lines().next().unwrap_or("")
        ))
    }
}

fn build_runtime_entities(
    config: &SupercellConfig,
    running: &AtomicBool,
) -> Result<Vec<RuntimeEntity>> {
    // Collect references of Flying entities so we can connect them in parallel.
    let flying_configs: Vec<_> = std::iter::once(&config.entities.ownship)
        .chain(config.entities.moving.iter())
        .collect();

    // Connect all JSBSim instances in parallel using scoped threads.
    let fdm_results: Vec<(usize, Result<Box<dyn FdmHandle + Send>>)> =
        std::thread::scope(|scope| {
            // Spawn threads eagerly into a vector so they run in parallel
            let mut handles = Vec::with_capacity(flying_configs.len());
            for (i, &entity_cfg) in flying_configs.iter().enumerate() {
                handles.push(scope.spawn(move || {
                    debug!(
                        entity_id = entity_cfg.base.entity_id,
                        name = %entity_cfg.base.name,
                        "connecting to JSBSim..."
                    );
                    let handle: Result<Box<dyn FdmHandle + Send>> =
                        JsbsimHandle::new_with_running(entity_cfg, running)
                            .map(|h| Box::new(h) as Box<dyn FdmHandle + Send>)
                            .with_context(|| {
                                format!("entity {}: FDM startup failed", entity_cfg.base.entity_id)
                            });
                    (i, handle)
                }));
            }
            let results: std::thread::Result<Vec<_>> = handles
                .into_iter()
                .map(std::thread::ScopedJoinHandle::join)
                .collect();
            results.expect("An FDM startup thread panicked")
        });

    // Index FDM handles by their spawn order.
    let mut fdm_map: std::collections::HashMap<usize, Box<dyn FdmHandle + Send>> =
        std::collections::HashMap::new();
    for (idx, result) in fdm_results {
        fdm_map.insert(idx, result?);
    }

    // Assemble RuntimeEntity list in config order.
    let mut entities: Vec<RuntimeEntity> = Vec::with_capacity(config.entities.iter_all().count());
    let mut flying_idx = 0;

    for entity_ref in config.entities.iter_all() {
        match entity_ref {
            EntityRef::Flying(entity_cfg) => {
                let mut handle = fdm_map
                    .remove(&flying_idx)
                    .expect("FDM handle missing for flying entity");
                flying_idx += 1;

                let waypoints = entity_cfg.flight_plan.clone().unwrap_or_default();

                let initial_state = match handle.read_state() {
                    Ok(mut s) => {
                        s.marking.clone_from(&entity_cfg.base.name);
                        s.is_static_entity = false;
                        s.has_waypoints = !waypoints.is_empty();
                        s
                    }
                    Err(e) => {
                        warn!(entity_id = entity_cfg.base.entity_id, error = %e,
                              "initial read_state failed — using zero kinematics");
                        EntityState {
                            entity_id: entity_cfg.base.entity_id,
                            site_id: entity_cfg.base.site_id,
                            application_id: entity_cfg.base.application_id,
                            force_id: entity_cfg.base.force_id,
                            entity_type: entity_cfg.base.entity_type.to_dis_entity_type(),
                            marking: entity_cfg.base.name.clone(),
                            is_static_entity: false,
                            has_waypoints: !waypoints.is_empty(),
                            ..EntityState::default()
                        }
                    }
                };

                debug!(
                    entity_id = entity_cfg.base.entity_id,
                    waypoints = waypoints.len(),
                    alt_m = format!("{:.1}", initial_state.altitude_m),
                    "JSBSim connected + trimmed"
                );

                entities.push(RuntimeEntity::Flying {
                    handle,
                    state: initial_state,
                    status: EntityStatus::Active,
                    waypoints,
                    active_wp: 0,
                    bridge: None,
                    prev_ecef_vel: None,
                    last_hdg_setpoint: None,
                    override_aggression: config
                        .flightgear
                        .as_ref()
                        .map_or(5.0, |fg| fg.override_aggression.clamp(1, 10) as f64),
                    autopilot_threshold: config
                        .flightgear
                        .as_ref()
                        .map_or(0.05, |fg| fg.autopilot_threshold),
                    override_timeout_secs: config
                        .flightgear
                        .as_ref()
                        .map_or(1.0, |fg| fg.override_timeout_secs),
                    last_fg_ctrls_at: None,
                });
            }
            EntityRef::Fixed(entity_cfg) => {
                // Config altitude is MSL; convert to HAE for DIS ECEF position.
                let alt_hae = entity_cfg.altitude_m + config.geoid_undulation_m;
                let state = EntityState {
                    latitude_deg: entity_cfg.latitude_deg,
                    longitude_deg: entity_cfg.longitude_deg,
                    altitude_m: alt_hae,
                    altitude_msl_m: entity_cfg.altitude_m,
                    entity_id: entity_cfg.base.entity_id,
                    site_id: entity_cfg.base.site_id,
                    application_id: entity_cfg.base.application_id,
                    force_id: entity_cfg.base.force_id,
                    entity_type: entity_cfg.base.entity_type.to_dis_entity_type(),
                    marking: entity_cfg.base.name.clone(),
                    is_static_entity: true,
                    ..EntityState::default()
                };
                entities.push(RuntimeEntity::Fixed {
                    state,
                    status: EntityStatus::Active,
                });
                debug!(entity_id = entity_cfg.base.entity_id, name = %entity_cfg.base.name,
                      "fixed entity constructed");
            }
        }
    }

    Ok(entities)
}

enum CliAction {
    /// Run the server using the provided configuration path.
    Run { config_path: String },
    /// Execute a simple health check against the local state file.
    HealthCheck { port: u16 },
    /// Print usage and exit successfully.
    Help,
}

fn parse_config_arg() -> Result<CliAction> {
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    let mut config_path = None;
    let mut health_check = false;
    let mut health_check_port: Option<u16> = None;

    while i < args.len() {
        if args[i] == "--health-check" {
            health_check = true;
            if i + 1 < args.len() && !args[i + 1].starts_with('-') {
                health_check_port = Some(
                    args[i + 1]
                        .parse()
                        .map_err(|e| anyhow!("invalid --health-check port value: {e}"))?,
                );
                i += 2;
            } else {
                i += 1;
            }
        } else if args[i] == "--help" || args[i] == "-h" {
            return Ok(CliAction::Help);
        } else if args[i] == "--config" {
            if i + 1 >= args.len() {
                return Err(anyhow!("--config flag requires a path argument"));
            }
            config_path = Some(args[i + 1].clone());
            i += 2;
        } else if !args[i].starts_with('-') && config_path.is_none() {
            // Allow positional config path for convenience if not flagged
            config_path = Some(args[i].clone());
            i += 1;
        } else {
            return Err(anyhow!("unknown argument: {}", args[i]));
        }
    }

    if health_check {
        let port = if let Some(p) = health_check_port {
            p
        } else {
            let env_port_str = std::env::var("SUPERCELL_ADMIN_PORT")
                .context("SUPERCELL_ADMIN_PORT environment variable must be set for --health-check without a port")?;
            env_port_str
                .parse::<u16>()
                .context("SUPERCELL_ADMIN_PORT must be a valid port number")?
        };
        Ok(CliAction::HealthCheck { port })
    } else if let Some(path) = config_path {
        Ok(CliAction::Run { config_path: path })
    } else {
        Err(anyhow!(
            "Usage: supercell --config <path/to/config.toml>\n       supercell --health-check [PORT]"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use supercell::config::{
        DisConfig, EntitiesConfig, EntityBaseConfig, EntityTypeConfig, FlyingEntityConfig,
        JsbsimConnectionMode, LaCalConfig, OmsConfig,
    };

    fn minimal_config() -> SupercellConfig {
        SupercellConfig {
            log_format: "text".to_string(),
            tick_hz: Some(5.0),
            time: None,
            settle_secs: 0.0,
            waypoint_threshold_m: 500.0,
            geoid_undulation_m: 0.0,
            log_level: "supercell=info".to_string(),
            otlp_endpoint: None,
            dis: DisConfig {
                multicast_addr: "127.0.0.1".to_string(),
                port: 3000,
                exercise_id: 1,
                ttl: Some(1),
                multicast_iface: None,
            },
            oms: None,
            admin_bind_addr: None,
            entities: EntitiesConfig {
                ownship: FlyingEntityConfig {
                    base: EntityBaseConfig {
                        entity_id: 1,
                        site_id: 1,
                        application_id: 1,
                        force_id: 1,
                        name: "flying-1".to_string(),
                        entity_type: EntityTypeConfig::default(),
                    },
                    aircraft: "c172x".to_string(),
                    jsbsim: JsbsimConnectionMode::Remote {
                        address: "127.0.0.1:5556".to_string(),
                    },
                    flight_plan: None,
                },
                moving: vec![],
                static_: vec![],
            },
            flightgear: None,
        }
    }

    #[test]
    fn handle_la_cal_startup_reports_not_configured_when_absent() {
        let config = minimal_config();

        let status = handle_la_cal_startup(&config).unwrap();

        assert_eq!(status, LaCalStartupStatus::NotConfigured);
    }

    #[test]
    fn handle_la_cal_startup_reports_ready_when_configured() {
        let mut config = minimal_config();
        config.oms = Some(OmsConfig {
            la_cal: Some(LaCalConfig {
                ws_url: "ws://127.0.0.1:8080/owp".to_string(),
                service_id: "supercell".to_string(),
                system_uuid: None,
                subsystem_uuid: None,
                namespace_uuid: None,
                system_name: None,
                subsystem_name: None,
                mission_name: None,
                classification: sleet_types::uci::v2_5::ClassificationEnum::U,
                owner_producer: sleet_types::uci::v2_5::OwnerProducerEnum::Usa,
                position_hz: 10.0,
                prd_hz: 2.0,
            }),
        });

        let status = handle_la_cal_startup(&config).unwrap();

        assert_eq!(status, LaCalStartupStatus::Ready);
    }

    #[test]
    fn build_runtime_entities_fails_when_jsbsim_unreachable() {
        let config = minimal_config();
        let running = AtomicBool::new(true);

        // Cancel quickly so connect retries don't run for 30s.
        let running_cancel = Arc::new(running);
        let cancel = Arc::clone(&running_cancel);
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(200));
            cancel.store(false, Ordering::SeqCst);
        });

        let result = build_runtime_entities(&config, running_cancel.as_ref());

        let Err(err) = result else {
            panic!("unreachable JSBSim must abort entity construction");
        };

        assert!(
            err.to_string().contains("FDM startup failed") || err.to_string().contains("cancelled"),
            "unexpected error: {err}"
        );
    }
}

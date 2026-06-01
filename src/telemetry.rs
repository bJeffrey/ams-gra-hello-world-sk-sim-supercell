use metrics::{describe_counter, describe_gauge, describe_histogram};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

/// Bootstrap tracing subscriber for early startup before config is loaded.
/// Returns a guard that keeps the thread-local subscriber active until dropped.
#[must_use = "The bootstrap guard must be held until final initialization"]
pub fn bootstrap() -> tracing::subscriber::DefaultGuard {
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("supercell=info"));

    tracing::subscriber::set_default(
        tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt::layer().with_target(true)),
    )
}

/// Resolve the `EnvFilter` based on precedence rules:
/// `RUST_LOG` environment variable > CLI flag > config file > default.
pub fn resolve_filter(env_val: Option<&str>, cli: Option<&str>, config: Option<&str>) -> EnvFilter {
    let candidate = env_val.or(cli).or(config).unwrap_or("supercell=info");
    EnvFilter::try_new(candidate).unwrap_or_else(|_| EnvFilter::new("supercell=info"))
}

/// Initialize tracing with an optional OTLP exporter and configurable output format.
///
/// This function must be called exactly once after the configuration file is parsed.
/// If `otlp_endpoint` is provided, the OTLP pipeline is installed (requires Tokio).
pub fn init_tracing(log_format: &str, log_level: Option<&str>, otlp_endpoint: Option<&str>) {
    let env_val = std::env::var("RUST_LOG").ok();
    let env_filter = resolve_filter(env_val.as_deref(), None, log_level);

    let is_json = log_format == "json";

    #[cfg(feature = "otlp")]
    if let Some(endpoint) = otlp_endpoint {
        use opentelemetry_otlp::WithExportConfig;

        let tracer = opentelemetry_otlp::new_pipeline()
            .tracing()
            .with_exporter(
                opentelemetry_otlp::new_exporter()
                    .tonic()
                    .with_endpoint(endpoint),
            )
            .install_batch(opentelemetry_sdk::runtime::Tokio)
            .expect("failed to install OTLP tracer");

        let otlp_layer = tracing_opentelemetry::layer().with_tracer(tracer);

        if is_json {
            tracing_subscriber::registry()
                .with(env_filter)
                .with(
                    fmt::layer()
                        .json()
                        .with_target(true)
                        .with_current_span(true),
                )
                .with(otlp_layer)
                .init();
        } else {
            tracing_subscriber::registry()
                .with(env_filter)
                .with(fmt::layer().with_target(true))
                .with(otlp_layer)
                .init();
        }
        return;
    }

    #[cfg(not(feature = "otlp"))]
    if otlp_endpoint.is_some() {
        tracing::warn!(
            "otlp_endpoint is configured but the 'otlp' feature is not enabled — traces will not be exported"
        );
    }

    // No OTLP — format only
    if is_json {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(
                fmt::layer()
                    .json()
                    .with_target(true)
                    .with_current_span(true),
            )
            .init();
    } else {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt::layer().with_target(true))
            .init();
    }
}

/// Initialize the Prometheus recorder and return a handle to render metrics.
pub fn init_metrics() -> Option<PrometheusHandle> {
    let builder = PrometheusBuilder::new();

    let handle = match builder.install_recorder() {
        Ok(h) => h,
        Err(e) => {
            tracing::warn!("metrics recorder already installed or failed, skipping: {e}");
            return None;
        }
    };

    describe_counter!("supercell_ticks_total", "Total simulation ticks executed");
    describe_gauge!(
        "supercell_entities_active",
        "Current number of active entities being simulated"
    );
    describe_counter!(
        "supercell_dis_pdus_published_total",
        "Total DIS EntityState PDUs published"
    );
    describe_counter!(
        "supercell_dis_publish_errors_total",
        "Total DIS publish errors"
    );
    describe_counter!(
        "supercell_owp_updates_total",
        "Total OWP state updates sent"
    );
    describe_counter!(
        "supercell_waypoints_reached_total",
        "Total waypoints reached by active entities"
    );
    describe_counter!(
        "supercell_fdm_errors_total",
        "Total FDM interaction errors (step/read)"
    );
    describe_histogram!(
        "supercell_tick_duration_seconds",
        "Duration of the simulation tick"
    );

    Some(handle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_filter_precedence() {
        // Env takes highest precedence
        let filter = resolve_filter(Some("env=debug"), Some("cli=info"), Some("cfg=warn"));
        assert_eq!(filter.to_string(), "env=debug");

        // CLI takes precedence over config
        let filter = resolve_filter(None, Some("cli=info"), Some("cfg=warn"));
        assert_eq!(filter.to_string(), "cli=info");

        // Config takes precedence over default
        let filter = resolve_filter(None, None, Some("cfg=warn"));
        assert_eq!(filter.to_string(), "cfg=warn");

        // Default
        let filter = resolve_filter(None, None, None);
        assert_eq!(filter.to_string(), "supercell=info");
    }
}

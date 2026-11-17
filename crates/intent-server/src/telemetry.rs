//! The `tracing` subscriber: JSON (default) or pretty, filtered by `INTENT_LOG`.

use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt as _, util::SubscriberInitExt as _};

use crate::config::LogFormat;

/// Installs the global subscriber. A bad filter is a startup error.
pub fn init(format: LogFormat, directives: &str) -> anyhow::Result<()> {
    let filter = EnvFilter::try_new(directives)?;
    let (json, pretty) = match format {
        LogFormat::Json => (
            Some(
                fmt::layer()
                    .json()
                    .flatten_event(true)
                    .with_current_span(true)
                    .with_span_list(false),
            ),
            None,
        ),
        LogFormat::Pretty => (None, Some(fmt::layer().compact())),
    };
    tracing_subscriber::registry()
        .with(filter)
        .with(json)
        .with(pretty)
        .try_init()?;
    Ok(())
}

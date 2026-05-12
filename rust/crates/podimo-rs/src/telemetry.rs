//! Tracing setup. Default format matches the Python service's
//! `LEVEL | TIMESTAMP | MESSAGE`. Set `PODIMO_LOG_JSON=true` to switch to JSON.

use std::env;

use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::time::FormatTime;
use tracing_subscriber::EnvFilter;

struct PodimoTimer;

impl FormatTime for PodimoTimer {
    fn format_time(&self, w: &mut Writer<'_>) -> std::fmt::Result {
        // Match Python's `%Y-%m-%dT%H:%M:%SZ`.
        write!(w, "{}", chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ"))
    }
}

pub fn init(debug: bool) {
    let default_level = if debug { "debug" } else { "info" };
    let filter =
        EnvFilter::try_from_env("RUST_LOG").unwrap_or_else(|_| EnvFilter::new(default_level));

    let json = env::var("PODIMO_LOG_JSON")
        .ok()
        .map(|v| {
            matches!(
                v.to_ascii_lowercase().as_str(),
                "true" | "1" | "t" | "y" | "yes"
            )
        })
        .unwrap_or(false);

    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false);

    if json {
        let _ = builder.json().try_init();
    } else {
        let _ = builder
            .with_timer(PodimoTimer)
            .with_level(true)
            .event_format(LineFormat)
            .try_init();
    }
}

struct LineFormat;

impl<S, N> tracing_subscriber::fmt::FormatEvent<S, N> for LineFormat
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
    N: for<'a> tracing_subscriber::fmt::FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &tracing_subscriber::fmt::FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &tracing::Event<'_>,
    ) -> std::fmt::Result {
        let meta = event.metadata();
        let level = meta.level().as_str();
        write!(writer, "{level} | ")?;
        PodimoTimer.format_time(&mut writer)?;
        write!(writer, " | ")?;
        ctx.field_format().format_fields(writer.by_ref(), event)?;
        writeln!(writer)
    }
}

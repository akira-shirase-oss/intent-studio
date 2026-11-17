//! `intent-provider-fixture --bind 127.0.0.1:59481 --mode unavailable`: the provider-down
//! harness as a process, for load runs. Switch modes with
//! `curl -X PUT -d '{"mode":"ok"}' -H 'content-type: application/json' http://127.0.0.1:59481/control/mode`.

use std::{net::SocketAddr, time::Duration};

use anyhow::Context as _;
use clap::Parser;
use intent_provider_fixture::{Fixture, Mode};

#[derive(Parser)]
#[command(about = "Intent Studio's notification provider fixture (development and test only)")]
struct Args {
    /// Address to listen on; keep it inside 127.0.0.1:59400–59499.
    #[arg(long, default_value = "127.0.0.1:59481")]
    bind: SocketAddr,
    /// Starting mode: `ok`, `unavailable`, `hang` or `accept_then_hang`.
    #[arg(long, default_value = "ok", value_parser = parse_mode)]
    mode: Mode,
    /// Seconds a hanging request is held before the fixture lets go.
    #[arg(long, default_value_t = 60)]
    hang_limit_secs: u64,
}

fn parse_mode(text: &str) -> Result<Mode, String> {
    serde_json::from_value(serde_json::Value::String(text.to_owned()))
        .map_err(|_| format!("unknown mode {text:?}"))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let listener = tokio::net::TcpListener::bind(args.bind)
        .await
        .with_context(|| format!("binding {}", args.bind))?;
    let fixture = Fixture::new(args.mode, Duration::from_secs(args.hang_limit_secs));
    let shutdown = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    eprintln!(
        "provider fixture listening on {} in mode {:?}",
        args.bind, args.mode
    );
    intent_provider_fixture::serve(listener, fixture, shutdown)
        .await
        .context("serving")
}

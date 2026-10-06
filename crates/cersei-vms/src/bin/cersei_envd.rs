//! `cersei-envd` binary — entry point for the in-VM daemon.
//!
//! Listens on the Unix socket given by `$CERSEI_ENVD_SOCKET` (default
//! `/run/cersei-envd.sock`).

use std::env;

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let socket =
        env::var("CERSEI_ENVD_SOCKET").unwrap_or_else(|_| "/run/cersei-envd.sock".to_string());
    cersei_vms::envd::run(&socket).await?;
    Ok(())
}

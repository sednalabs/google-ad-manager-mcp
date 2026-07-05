use anyhow::Result;
use clap::Parser;
use mcp_toolkit_core::tool_schema::tool_schema_snapshot_value;
use tracing_subscriber::EnvFilter;

use google_ad_manager_mcp::{AdManagerServer, Cli, CliCommand, Settings, auth_ux};

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("google-ad-manager-mcp failed to start: {err}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    init_tracing();
    let settings = Settings::from_cli(Cli::parse())?;

    if let Some(command) = settings.command.clone() {
        match command {
            CliCommand::Serve => {}
            CliCommand::Auth(auth) => {
                auth_ux::run_auth_command(&settings, &auth.command).await?;
                return Ok(());
            }
        }
    }

    let server = AdManagerServer::new(settings.clone())?;

    if settings.print_tools {
        println!("{}", serde_json::to_string_pretty(&server.tool_names())?);
        return Ok(());
    }

    if settings.print_tool_schema {
        println!(
            "{}",
            serde_json::to_string_pretty(&tool_schema_snapshot_value(
                &server.tool_schema_snapshot()
            )?)?
        );
        return Ok(());
    }

    mcp_toolkit_observability::emit_event(
        mcp_toolkit_observability::Level::INFO,
        "google_ad_manager_mcp.startup",
        &mcp_toolkit_observability::EventContext::new(),
        &[
            mcp_toolkit_observability::safe_text("transport", "stdio"),
            mcp_toolkit_observability::safe_text("scope", &settings.scope),
        ],
    );

    mcp_toolkit::server::stdio::serve_stdio(server).await?;
    Ok(())
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .with_writer(std::io::stderr)
        .try_init();
}

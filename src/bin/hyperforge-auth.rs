//! Hyperforge Auth Hub
//!
//! Simple secret management hub using YAML storage.
//!
//! Usage:
//!   hyperforge-auth [--port PORT]
//!
//! Default port: 4445

use clap::Parser;
use hub_core::plexus::DynamicHub;
use hub_transport::TransportServer;
use hyperforge::auth_hub::AuthHub;
use std::sync::Arc;
use tracing::{info, error};

#[derive(Parser, Debug)]
#[command(name = "hyperforge-auth")]
#[command(about = "Hyperforge Auth Hub - Secret management", long_about = None)]
struct Args {
    /// Port to listen on
    #[arg(short, long, default_value = "4445")]
    port: u16,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"))
        )
        .init();

    let args = Args::parse();

    info!("Starting Hyperforge Auth Hub");
    info!("Storage: ~/.config/hyperforge/secrets.yaml");

    // Create auth hub
    let auth_hub = match AuthHub::new().await {
        Ok(hub) => hub,
        Err(e) => {
            error!("Failed to create auth hub: {}", e);
            std::process::exit(1);
        }
    };

    info!("Auth hub initialized");

    // Create auth hub with unique namespace
    let auth = Arc::new(
        DynamicHub::new("auth")
            .register(auth_hub)
    );

    info!("Auth hub started");
    info!("  Namespace: auth");
    info!("  Activation: auth");
    info!("  Version: 1.0.0");
    info!("  Description: Simple secret management");

    // Configure transport server
    let rpc_converter = |arc: Arc<DynamicHub>| {
        DynamicHub::arc_into_rpc_module(arc)
            .map_err(|e| anyhow::anyhow!("Failed to create RPC module: {}", e))
    };

    let builder = TransportServer::builder(auth, rpc_converter)
        .with_websocket(args.port);

    info!("Server started");
    info!("  WebSocket: ws://127.0.0.1:{}", args.port);
    info!("");
    info!("Usage:");
    info!("  synapse -P {} auth auth set_secret --path <PATH> --value <VALUE>", args.port);
    info!("  synapse -P {} auth auth get_secret --path <PATH>", args.port);
    info!("  synapse -P {} auth auth list_secrets --prefix <PREFIX>", args.port);
    info!("");

    // Start the transport server
    builder.build().await?.serve().await
}

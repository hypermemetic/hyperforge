use clap::Parser;
use plexus_core::plexus::DynamicHub;
use plexus_transport::TransportServer;
use hyperforge::HyperforgeHub;
use std::sync::Arc;

/// CLI arguments for hyperforge standalone server
#[derive(Parser, Debug)]
#[command(name = "hyperforge")]
#[command(about = "Hyperforge standalone server - JSON-RPC over WebSocket or stdio")]
struct Args {
    /// Run in stdio mode for MCP compatibility (line-delimited JSON-RPC over stdin/stdout)
    #[arg(long)]
    stdio: bool,

    /// Port for WebSocket server (ignored in stdio mode)
    #[arg(short, long, default_value = "4446")]
    port: u16,

    /// Enable MCP HTTP server (on port + 1)
    #[arg(long)]
    mcp: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Parse CLI arguments
    let args = Args::parse();

    // Initialize tracing with filtering
    let filter = if args.stdio {
        tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            tracing_subscriber::EnvFilter::new("hyperforge=warn,jsonrpsee=warn")
        })
    } else {
        tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            #[cfg(debug_assertions)]
            let default_filter = "warn,hyperforge=trace";
            #[cfg(not(debug_assertions))]
            let default_filter = "warn,hyperforge=debug";
            tracing_subscriber::EnvFilter::new(default_filter)
        })
    };

    // In stdio mode, send logs to stderr to keep stdout clean for JSON-RPC
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();

    tracing::info!("Starting hyperforge at {}", chrono::Utc::now());

    // Create lforge hub (DynamicHub with "lforge" namespace, hyperforge activation registered)
    let lforge = Arc::new(
        DynamicHub::new("lforge")
            .register(HyperforgeHub::new())
    );

    // Log activation info
    tracing::info!("LFORGE2 initialized");
    tracing::info!("  Namespace: lforge");
    tracing::info!("  Activation: hyperforge");
    tracing::info!("  Version: {}", env!("CARGO_PKG_VERSION"));
    tracing::info!("  Description: Multi-forge repository management");

    // Configure transport server
    let rpc_converter = |arc: Arc<DynamicHub>| {
        DynamicHub::arc_into_rpc_module(arc)
            .map_err(|e| anyhow::anyhow!("Failed to create RPC module: {}", e))
    };

    let mut builder = TransportServer::builder(lforge, rpc_converter);

    // Add requested transports
    if args.stdio {
        builder = builder.with_stdio();
    } else {
        builder = builder.with_websocket(args.port);

        if args.mcp {
            builder = builder.with_mcp_http(args.port + 1);
        }
    }

    // Log what we're starting
    if args.stdio {
        tracing::info!("Starting stdio transport (MCP-compatible)");
    } else {
        tracing::info!("LFORGE2 started");
        tracing::info!("  WebSocket: ws://127.0.0.1:{}", args.port);
        if args.mcp {
            tracing::info!("  MCP HTTP:  http://127.0.0.1:{}/mcp", args.port + 1);
        }
        tracing::info!("");
        tracing::info!("Usage:");
        tracing::info!("  synapse -p {} lforge hyperforge status", args.port);
        tracing::info!("  synapse -p {} lforge hyperforge version", args.port);
    }

    // Start the transport server
    builder.build().await?.serve().await
}

// Main entry point for the upsclr-server application.
// Sets up the Tokio runtime, initializes services (PluginManager, InstanceManager),
// configures the Axum router, and starts the HTTP server.

mod error;
mod handlers;
mod headers;
mod instance_manager;
mod models;
mod plugin_ffi;
mod plugin_manager;

use axum::{
    Extension, Router,
    extract::DefaultBodyLimit,
    middleware,
    routing::{delete, get, post},
};
use clap::Parser;
use instance_manager::InstanceManager;
use plugin_manager::PluginManager;
use socket2::{Domain, Protocol, Socket, Type};
use std::{
    net::SocketAddr,
    sync::{Arc, RwLock},
};
use tokio::signal;
use tower_http::{
    trace::{DefaultMakeSpan, TraceLayer}, // For detailed request logging
};
use tracing::Level;

/// Command line arguments for upsclr-server
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct AppConfig {
    /// Hostname/IP to bind the server to.
    /// If this option is specified without value, it will default to "*", meaning the server will listen on all interfaces.
    #[arg(long, env = "UPSCLR_SERVER_HOST", default_value = "localhost", num_args = 0..=1, default_missing_value = "*")]
    host: String,

    /// Port number to listen on.
    #[arg(short, long, env = "UPSCLR_SERVER_PORT", default_value_t = 6795)]
    port: u16,

    /// Directory containing plugin shared libraries.
    #[arg(long, env = "UPSCLR_SERVER_PLUGINS_DIR", default_value = "plugins")]
    plugins_dir: String,

    /// Disable User-Agent validation that requires User-Agent to start with 'Upsclr/' or 'Upsclr-'.
    /// By default, User-Agent validation is enabled for security.
    #[arg(long, env = "UPSCLR_SERVER_ALLOW_ANY_USER_AGENT", action = clap::ArgAction::SetTrue)]
    allow_any_user_agent: bool,

    /// Disable Upsclr-Request header validation that requires the header to be non-empty.
    /// By default, Upsclr-Request header validation is enabled for security.
    #[arg(long, env = "UPSCLR_SERVER_ALLOW_MISSING_UPSCLR_REQUEST", action = clap::ArgAction::SetTrue)]
    allow_missing_upsclr_request: bool,
}

#[tokio::main]
async fn main() {
    // Parse command line args and environment variables
    let config = AppConfig::parse();

    // Initialize tracing subscriber for structured logging.
    // Logs will go to stdout. Adjust level and format as needed.
    tracing_subscriber::fmt()
        .with_max_level(Level::INFO) // Set to DEBUG for more verbose FFI/plugin load logs
        .with_target(true) // Include module path in logs
        .with_file(true) // Include source file name
        .with_line_number(true) // Include line numbers
        .init();

    tracing::info!("Starting upsclr-server...");

    // Configuration: Path to the directory containing plugin shared libraries.
    let plugins_directory = &config.plugins_dir;
    tracing::info!("Plugin directory set to: {}", plugins_directory);

    // --- Initialize PluginManager ---
    // This operation is `unsafe` because it involves loading dynamic libraries (FFI).
    // It should be one of the first things done, as plugins are core to functionality.
    let plugin_manager = unsafe {
        PluginManager::new(plugins_directory)
    }.unwrap_or_else(|err| {
            // If plugin loading fails critically, the server might be useless.
            tracing::error!(
                "FATAL: Failed to initialize PluginManager: {:?}. Server cannot operate without plugins.",
                err
            );
            eprintln!("FATAL: Plugin initialization failed. See logs for details. Exiting.");
            std::process::exit(1); // Exit if no plugins can be loaded.
        }
    );
    tracing::info!(
        "PluginManager initialized. Loaded {} plugin(s).",
        plugin_manager.count_plugins()
    );
    if plugin_manager.is_empty() {
        tracing::warn!(
            "No plugins were loaded. The server will run but may have no upscaling capabilities."
        );
    }

    let plugin_manager_arc = Arc::new(RwLock::new(plugin_manager));

    // --- Initialize InstanceManager ---
    let instance_manager_arc = Arc::new(RwLock::new(InstanceManager::default()));
    tracing::info!("InstanceManager initialized.");

    // --- Configure Security Validation ---
    let security_config = handlers::SecurityConfig {
        require_user_agent_prefix: !config.allow_any_user_agent,
        require_upsclr_request_header: !config.allow_missing_upsclr_request,
    };

    // Log the security configuration
    if security_config.require_user_agent_prefix || security_config.require_upsclr_request_header {
        tracing::info!("Request security validation enabled:");
        if security_config.require_user_agent_prefix {
            tracing::info!("  - User-Agent must start with 'Upsclr/' or 'Upsclr-'");
        }
        if security_config.require_upsclr_request_header {
            tracing::info!("  - Upsclr-Request header must be non-empty");
        }
    } else {
        tracing::warn!("Request security validation is disabled - all requests will be allowed");
    }

    // --- Build Axum Application Router ---
    // Define routes and associate them with their respective handler functions.
    // Also, apply middleware layers.

    // Create a router with combined state for handlers needing both managers
    let mut app = Router::new()
        // Plugin and Engine discovery (only needs PluginManager)
        .route("/plugins", get(handlers::get_plugins))
        // Instance management endpoints
        .route(
            "/instances",
            get(handlers::list_instances).post(handlers::create_instance_handler),
        )
        .route(
            "/instances/{uuid}",
            delete(handlers::delete_instance_handler),
        )
        // Instance operations
        .route(
            "/instances/{uuid}/preload",
            post(handlers::preload_instance),
        )
        .route("/instances/{uuid}/upscale", post(handlers::upscale_image))
        // Reset endpoint
        .route("/reset", post(handlers::reset))
        // Apply a layer to limit the maximum size of request bodies (e.g., for image uploads).
        .layer(DefaultBodyLimit::max(handlers::MAX_IMAGE_SIZE_BYTES));

    // Apply security validation middleware if any restrictions are configured
    if security_config.require_user_agent_prefix || security_config.require_upsclr_request_header {
        app = app
            .layer(middleware::from_fn(handlers::validate_request_security))
            .layer(Extension(security_config));
    }

    let app = app
        // Add a TraceLayer for logging HTTP request and response details.
        .layer(
            TraceLayer::new_for_http().make_span_with(DefaultMakeSpan::new().level(Level::INFO)), // Log at INFO level
        )
        // Provide the shared state
        .with_state((plugin_manager_arc.clone(), instance_manager_arc.clone()));

    tracing::info!("Axum router configured.");

    // --- Start HTTP Server ---
    let listener = match create_listener(&config.host, config.port).await {
        Ok((addr, l)) => {
            tracing::info!("Server successfully bound. Listening on {}", addr);
            l
        }
        Err(e) => {
            tracing::error!("FATAL: Failed to bind server: {}", e);
            eprintln!("FATAL: Could not bind server. Error: {}. Exiting.", e);
            std::process::exit(1);
        }
    };

    // Run the server.
    if let Err(e) = axum::serve(listener, app.into_make_service())
        .with_graceful_shutdown(shutdown_signal())
        .await
    {
        tracing::error!("Server run error: {}", e);
        eprintln!("ERROR: Server shut down unexpectedly. Error: {}", e);
    }

    tracing::info!("upsclr-server has shut down.");

    instance_manager_arc.write().unwrap().cleanup(true);
    plugin_manager_arc.write().unwrap().cleanup(true);
}

async fn create_listener(
    host: &str,
    port: u16,
) -> std::io::Result<(String, tokio::net::TcpListener)> {
    if host == "*" {
        fn create_ipv6_dual_stack_wildcard_listener(
            port: u16,
        ) -> std::io::Result<(String, tokio::net::TcpListener)> {
            let str_addr = format!("[::]:{}", port);
            let addr: SocketAddr = str_addr.parse().unwrap();

            tracing::info!(
                "Attempting to bind server to {}... (IPv6 + IPv4 dual-stack)",
                str_addr
            );

            // Try to create an IPv6 socket (this will fail if IPv6 is not supported)
            let socket = Socket::new(Domain::IPV6, Type::STREAM, Some(Protocol::TCP))?;

            // Try to make it dual-stack (this might fail on some systems)
            if let Err(e) = socket.set_only_v6(false) {
                tracing::warn!(
                    "Warning: Failed to set dual-stack mode for IPv6 socket: {}. Continuing anyway.",
                    e
                );
                // Continue anyway, as some systems might still work
            }

            socket.set_reuse_address(true)?;
            socket.bind(&addr.into())?;
            socket.listen(1024)?;

            // Make it non-blocking for tokio
            socket.set_nonblocking(true)?;

            // Convert to tokio listener
            let std_listener: std::net::TcpListener = socket.into();
            let tokio_listener = tokio::net::TcpListener::from_std(std_listener)?;

            Ok((str_addr, tokio_listener))
        }

        fn create_wildcard_listener(
            port: u16,
        ) -> std::io::Result<(String, tokio::net::TcpListener)> {
            // Try to create an IPv6 socket first
            // This will work on systems that support IPv6, and if it also supports dual-stack, it will bind to both IPv4 and IPv6.
            let ipv6_listener = create_ipv6_dual_stack_wildcard_listener(port);
            if ipv6_listener.is_ok() {
                return ipv6_listener;
            }

            tracing::warn!("Warning: Failed to bind IPv6 listener. Attempting IPv4 only.");

            let str_addr = format!("0.0.0.0:{}", port);
            let addr: SocketAddr = str_addr.parse().unwrap();

            tracing::info!("Attempting to bind server to {}... (IPv4)", str_addr);

            // Try to create an IPv4 socket
            let socket = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;

            socket.set_reuse_address(true)?;
            socket.bind(&addr.into())?;
            socket.listen(1024)?;

            // Make it non-blocking for tokio
            socket.set_nonblocking(true)?;

            let std_listener: std::net::TcpListener = socket.into();
            let tokio_listener = tokio::net::TcpListener::from_std(std_listener)?;

            Ok((str_addr, tokio_listener))
        }

        return create_wildcard_listener(port);
    }

    let addr = format!("{}:{}", host, port);
    tracing::info!("Attempting to bind server to {}...", addr);

    let tokio_listener = tokio::net::TcpListener::bind(&addr).await?;

    Ok((addr, tokio_listener))
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

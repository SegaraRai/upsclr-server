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
    Router,
    extract::DefaultBodyLimit,
    routing::{delete, get, post},
};
use instance_manager::InstanceManager;
use plugin_manager::PluginManager;
use std::sync::{Arc, Mutex};
use tokio::signal;
use tower_http::{
    cors::CorsLayer,                      // For Cross-Origin Resource Sharing
    trace::{DefaultMakeSpan, TraceLayer}, // For detailed request logging
};
use tracing::Level;

#[tokio::main]
async fn main() {
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
    // Consider making this configurable via environment variable or config file.
    let plugins_directory = "plugins";
    tracing::info!("Plugin directory set to: {}", plugins_directory);

    // --- Initialize PluginManager ---
    // This operation is `unsafe` because it involves loading dynamic libraries (FFI).
    // It should be one of the first things done, as plugins are core to functionality.
    let plugin_manager_arc = match unsafe { PluginManager::new(plugins_directory) } {
        Ok(pm) => Arc::new(pm),
        Err(e) => {
            // If plugin loading fails critically, the server might be useless.
            tracing::error!(
                "FATAL: Failed to initialize PluginManager: {:?}. Server cannot operate without plugins.",
                e
            );
            eprintln!("FATAL: Plugin initialization failed. See logs for details. Exiting.");
            std::process::exit(1); // Exit if no plugins can be loaded.
        }
    };
    tracing::info!(
        "PluginManager initialized. Loaded {} plugin(s).",
        plugin_manager_arc.plugins.len()
    );
    if plugin_manager_arc.plugins.is_empty() {
        tracing::warn!(
            "No plugins were loaded. The server will run but may have no upscaling capabilities."
        );
    }

    // --- Initialize InstanceManager ---
    // Wrapped in Arc<Mutex> for shared, mutable access across concurrent Axum tasks.
    let instance_manager_arc = Arc::new(Mutex::new(InstanceManager::default()));
    tracing::info!("InstanceManager initialized.");

    // --- Build Axum Application Router ---
    // Define routes and associate them with their respective handler functions.
    // Also, apply middleware layers.

    // Create separate routers for different state types
    // Router for handlers that need only PluginManager
    let app_router = Router::new()
        // Plugin and Engine discovery (only needs PluginManager)
        .route("/plugins", get(handlers::get_plugins))
        // Apply a layer to limit the maximum size of request bodies (e.g., for image uploads).
        .layer(DefaultBodyLimit::max(handlers::MAX_IMAGE_SIZE_BYTES))
        // Add CORS layer for broader client compatibility (e.g., web frontends from different origins).
        // Configure this layer according to your security requirements.
        .layer(CorsLayer::permissive()) // Example: allows all origins. Restrict in production.
        // Add a TraceLayer for logging HTTP request and response details.
        .layer(
            TraceLayer::new_for_http().make_span_with(DefaultMakeSpan::new().level(Level::INFO)), // Log at INFO level
        )
        // Provide the shared state
        .with_state(plugin_manager_arc.clone());

    // Create a router with combined state for handlers needing both managers
    // We need to use the tuple state for handlers needing both managers
    let combined_state = (plugin_manager_arc.clone(), instance_manager_arc.clone());
    let instances_router = Router::new()
        // Instance management endpoints
        .route(
            "/",
            get(handlers::list_instances).post(handlers::create_instance_handler),
        )
        .route("/{uuid}", delete(handlers::delete_instance_handler))
        // Instance operations
        .route("/{uuid}/preload", post(handlers::preload_instance))
        .route("/{uuid}/upscale", post(handlers::upscale_image))
        .with_state(combined_state);

    // Merge the routers
    let app = app_router.nest("/instances", instances_router);

    tracing::info!("Axum router configured.");

    // --- Start HTTP Server ---
    // Bind to a TCP address and serve the Axum application.
    let server_address = "0.0.0.0:3000"; // Listen on all interfaces, port 3000.
    tracing::info!("Attempting to bind server to {}...", server_address);

    let listener = match tokio::net::TcpListener::bind(server_address).await {
        Ok(l) => {
            tracing::info!("Server successfully bound. Listening on {}", server_address);
            l
        }
        Err(e) => {
            tracing::error!(
                "FATAL: Failed to bind server to address {}: {}",
                server_address,
                e
            );
            eprintln!(
                "FATAL: Could not bind to {}. Error: {}. Exiting.",
                server_address, e
            );
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

    instance_manager_arc.lock().unwrap().cleanup();
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

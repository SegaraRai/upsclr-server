use super::{MAX_IMAGE_SIZE_BYTES, SharedPluginManager, handlers};
use axum::{
    Router,
    extract::DefaultBodyLimit,
    routing::{IntoMakeService, delete, get, post},
};
use tower_http::{
    cors::CorsLayer,
    trace::{DefaultMakeSpan, TraceLayer},
};
use tracing::Level;

pub fn create_app(plugin_manager: SharedPluginManager) -> IntoMakeService<axum::Router<()>> {
    // Configure the router with all API endpoints
    Router::new()
        // Plugin and Engine discovery
        .route("/plugins", get(handlers::get_plugins))
        // Instance management endpoints
        .route(
            "/instances",
            get(handlers::list_instances).post(handlers::create_instance),
        )
        .route("/instances/{uuid}", delete(handlers::delete_instance))
        // Instance operations
        .route(
            "/instances/{uuid}/preload",
            post(handlers::preload_instance),
        )
        .route("/instances/{uuid}/upscale", post(handlers::upscale_image))
        // Apply a layer to limit the maximum size of request bodies
        .layer(DefaultBodyLimit::max(MAX_IMAGE_SIZE_BYTES))
        // Add CORS layer for broader client compatibility
        .layer(CorsLayer::permissive())
        // Add tracing for HTTP requests and responses
        .layer(TraceLayer::new_for_http().make_span_with(DefaultMakeSpan::new().level(Level::INFO)))
        // Provide the shared state
        .with_state(plugin_manager)
        .into_make_service()
}

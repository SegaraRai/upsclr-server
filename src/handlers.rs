// Contains the Axum handler functions for each API endpoint.
// These handlers process requests, interact with managers, and generate responses.
//
// NOTE: Many of these handlers are now replaced by versions in combined_state_handlers.rs
// that use a tuple state (SharedPluginManager, SharedInstanceManager).
// The versions in this file are kept for reference.

use crate::{
    error::{lock_mutex_app_error, AppError},
    headers,
    instance_manager::InstanceManager,
    models::*,
    plugin_ffi,
    plugin_manager::PluginManager,
};
use axum::{
    extract::{Multipart, Path, Query, State},
    http::{header, StatusCode},
    response::{AppendHeaders, IntoResponse, Response},
    Json,
};
use axum_extra::TypedHeader;
use byteorder::{ByteOrder, LittleEndian}; // For parsing x-raw-bitmap header
use image::{DynamicImage, ImageFormat};
use std::ffi::CString;
use std::io::Cursor;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

// --- Shared State Type Aliases (for convenience in handler signatures) ---
#[allow(unused)]
pub type SharedPluginManager = Arc<PluginManager>;
// InstanceManager methods take `&mut self` for `create_instance` and `delete_instance`,
// so it needs a Mutex when shared across Axum's concurrent tasks.
#[allow(unused)]
pub type SharedInstanceManager = Arc<Mutex<InstanceManager>>;

pub const MAX_IMAGE_SIZE_BYTES: usize = 100 * 1024 * 1024; // Example: 100MB limit for input image.

// --- GET /plugins ---
// Lists all loaded plugins and their available engines.
pub async fn get_plugins(
    State(plugin_manager): State<SharedPluginManager>,
) -> Result<Json<Vec<PluginDescriptionResponse>>, AppError> {
    let descriptions: Vec<PluginDescriptionResponse> = plugin_manager
        .plugins
        .iter()
        .map(|plugin_arc| PluginDescriptionResponse {
            plugin_info: plugin_arc.plugin_info.clone(),
            engines: plugin_arc.engines_info.clone(),
        })
        .collect();
    tracing::debug!("Returning {} plugin descriptions.", descriptions.len());
    Ok(Json(descriptions))
}

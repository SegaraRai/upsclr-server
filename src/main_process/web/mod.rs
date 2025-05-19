// Web server module for the main process
// Handles HTTP API endpoints for plugin and upscaling operations

mod app;
mod error;
mod extract_request_data;
mod handlers;
mod headers;
mod image_codec;
mod listeners;
mod models;

pub use app::create_app;
pub use listeners::create_listener;

use crate::main_process::plugin_manager::PluginManager;
use std::sync::Arc;
use tokio::sync::RwLock;

// Maximum allowed size for image upload requests
pub const MAX_IMAGE_SIZE_BYTES: usize = 100 * 1024 * 1024; // 100MB

pub type SharedPluginManager = Arc<RwLock<PluginManager>>;

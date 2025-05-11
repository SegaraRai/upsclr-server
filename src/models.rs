// Defines data structures for API request and response bodies,
// using Serde for JSON serialization and deserialization.

use crate::plugin_manager::{EngineInfo, PluginInfo}; // Use the Rust-native structs from plugin_manager
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// Describes a plugin and its available engines for the GET /plugins endpoint.
#[derive(Serialize, Debug)]
pub struct PluginDescriptionResponse {
    // Embeds fields from PluginInfo (name, version, description) directly into this struct's JSON.
    #[serde(flatten)]
    pub plugin_info: PluginInfo,
    // Lists all engines provided by this plugin.
    pub engines: Vec<EngineInfo>,
}

// Request body structure for POST /instances to create a new engine instance.
#[derive(Deserialize, Debug)]
pub struct CreateInstanceRequest {
    // Unique identifier for the plugin.
    pub plugin_id: String,
    // Name of the specific engine within the plugin to instantiate.
    pub engine_name: String,
    // Engine-specific configuration, passed as a flexible JSON value.
    // The plugin itself (via its JSON schema) defines the structure of this config.
    pub config: serde_json::Value,
}

// Response structure for POST /instances.
#[derive(Serialize, Debug)]
pub struct CreateInstanceResponse {
    // The unique UUID assigned to the newly created (or validated) engine instance.
    // Will be None if dry_run was true and no instance was actually created.
    pub instance_id: Option<Uuid>,
    // Provides the result of the configuration validation.
    // This is always present:
    // - For dry_run=true, it's the primary result.
    // - For dry_run=false, it contains any warnings or confirms validity before creation.
    pub validation: Option<ValidationResultDesc>, // Kept as Option in case validation is skipped or yields no info
}

// Describes the outcome of an engine configuration validation attempt.
// This structure is derived from the plugin's FFI validation result.
#[derive(Serialize, Deserialize, Debug, Clone)] // Added Clone for easier handling
pub struct ValidationResultDesc {
    pub is_valid: bool,
    pub error_count: usize,
    pub error_messages: Vec<String>,
    pub warning_count: usize,
    pub warning_messages: Vec<String>,
}

// Query parameters for endpoints that accept a 'scale' factor,
// such as /preload and /upscale.
#[derive(Deserialize, Debug)]
pub struct ScaleQueryParam {
    // The desired upscale factor (e.g., 2, 3, or 4).
    pub scale: i32,
}

// Query parameters specific to the POST /instances endpoint.
#[derive(Deserialize, Debug)]
pub struct CreateInstanceQuery {
    // If Some(true), the server should only validate the configuration
    // without creating an actual engine instance.
    // If Some(false) or None, an instance is created if validation passes.
    // Defaults to false if not provided.
    #[serde(default, deserialize_with = "deserialize_bool_from_int_optional_query")]
    pub dry_run: Option<bool>,
}

// Custom deserializer for dry_run query parameter (0, 1, or missing).
pub fn deserialize_bool_from_int_optional_query<'de, D>(
    deserializer: D,
) -> Result<Option<bool>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    // This handles cases where dry_run might be sent as a string "0" or "1" from query params.
    // Axum's Query extractor is pretty good, but explicit handling can be robust.
    // For simplicity with Axum's Query, often just Option<bool> or Option<u8> and manual check is fine too.
    // Here, assuming it comes as a number or can be parsed into one.
    let s: Option<String> = Option::deserialize(deserializer)?;
    match s.as_deref() {
        Some("1") => Ok(Some(true)),
        Some("true") => Ok(Some(true)),
        Some("0") => Ok(Some(false)),
        Some("false") => Ok(Some(false)),
        None => Ok(None), // Parameter not present
        Some(other) => Err(serde::de::Error::invalid_value(
            serde::de::Unexpected::Str(other),
            &"0, 1, true, false, or not present",
        )),
    }
}

// Simplified information for listing instances via GET /instances.
#[derive(Serialize, Debug)]
pub struct InstanceInfoForList {
    pub instance_id: Uuid,
    pub plugin_id: String,
    pub plugin_name: String,
    pub engine_name: String,
    // Potentially include more details like creation time or basic config summary if needed.
}

// API-specific data models for the web server

use crate::models::{EngineInfo, PluginInfo};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Plugin description response returned by the API
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PluginDescription {
    pub plugin_info: PluginInfo,
    pub engines: Vec<EngineInfo>,
}

/// Request to create a new engine instance
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CreateInstanceRequest {
    pub plugin_id: Uuid,
    pub engine_name: String,
    pub config: serde_json::Value,
}

/// Query parameters for instance creation
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CreateInstanceQuery {
    pub dry_run: Option<bool>,
}

/// Response to a create instance request
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CreateInstanceResponse {
    pub instance_id: Option<Uuid>,
    pub validation: Option<ValidationResultDesc>,
}

/// Validation result description
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ValidationResultDesc {
    pub is_valid: bool,
    pub error_count: usize,
    pub error_messages: Vec<String>,
    pub warning_count: usize,
    pub warning_messages: Vec<String>,
}

/// Query parameters for upscale and preload operations
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ScaleQueryParam {
    pub scale: u32,
}

/// Instance info for listing instances
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct InstanceInfoForList {
    pub instance_id: Uuid,
    pub plugin_id: Uuid,
    pub plugin_name: String,
    pub engine_name: String,
    pub uptime_ms: u64,
}

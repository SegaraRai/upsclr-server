// API-specific data models for the web server

use std::path::PathBuf;

use crate::models::{EngineInfo, EngineInstance, PluginInfo};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ApiEngineInfo {
    pub name: String,
    pub description: String,
    pub version: String,
    pub config_schema: serde_json::Value,
    pub plugin_id: Uuid,
}

impl TryFrom<EngineInfo> for ApiEngineInfo {
    type Error = serde_json::Error;

    fn try_from(engine_info: EngineInfo) -> Result<Self, Self::Error> {
        Ok(ApiEngineInfo {
            name: engine_info.name,
            description: engine_info.description,
            version: engine_info.version,
            config_schema: serde_json::from_str(&engine_info.config_schema)?,
            plugin_id: engine_info.plugin_id,
        })
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ApiEngineInstance {
    pub instance_id: Uuid,
    pub plugin_id: Uuid,
    pub engine_name: String,
    pub engine_config: serde_json::Value,
    pub uptime_ms: u64,
}

impl TryFrom<EngineInstance> for ApiEngineInstance {
    type Error = serde_json::Error;

    fn try_from(instance: EngineInstance) -> Result<Self, Self::Error> {
        Ok(ApiEngineInstance {
            instance_id: instance.config.instance_id,
            plugin_id: instance.config.plugin_id,
            engine_name: instance.config.engine_name,
            engine_config: serde_json::from_str(&instance.config.engine_config)?,
            uptime_ms: instance.uptime_ms,
        })
    }
}

/// Plugin description response returned by the API
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ApiPluginInfo {
    pub plugin_id: Uuid,
    pub path: PathBuf,
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub supported_engines: Vec<ApiEngineInfo>,
    pub engine_instances: Vec<ApiEngineInstance>,
}

impl TryFrom<PluginInfo> for ApiPluginInfo {
    type Error = serde_json::Error;

    fn try_from(plugin_info: PluginInfo) -> Result<Self, Self::Error> {
        Ok(ApiPluginInfo {
            plugin_id: plugin_info.plugin_id,
            path: plugin_info.path,
            name: plugin_info.name,
            version: plugin_info.version,
            author: plugin_info.author,
            description: plugin_info.description,
            supported_engines: plugin_info
                .supported_engines
                .into_iter()
                .map(ApiEngineInfo::try_from)
                .collect::<Result<Vec<_>, _>>()?,
            engine_instances: plugin_info
                .engine_instances
                .into_iter()
                .map(ApiEngineInstance::try_from)
                .collect::<Result<Vec<_>, _>>()?,
        })
    }
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

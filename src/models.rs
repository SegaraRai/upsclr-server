use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

/// Represents error types that can occur in the plugin host
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PluginHostError {
    PluginNotFound(String),
    PluginLoadError(String),
    EngineNotFound(String),
    EngineCreationFailed(String),
    EngineInstanceNotFound(Uuid),
    OperationFailed(String),
    InvalidParameter(String),
    UnsupportedOperation(String),
    ResourceLimitExceeded(String),
    Timeout(String),
    InternalError(String),
}

/// Represents the color format used in image processing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ColorFormat {
    Rgb,
    Bgr,
}

/// Information about a loaded plugin
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInfo {
    pub plugin_id: Uuid,
    pub path: PathBuf,
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub supported_engines: Vec<EngineInfo>,
    pub engine_instances: Vec<EngineInstance>,
}

/// Configuration for an upscaling engine instance
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineInfo {
    pub name: String,
    pub description: String,
    pub version: String,
    pub config_schema: String, // JSON string, since bincode cannot serde_json::Value
    pub plugin_id: Uuid,
}

/// Result of validating an engine configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineConfigValidationResult {
    pub is_valid: bool,
    pub warning_messages: Vec<String>,
    pub error_messages: Vec<String>,
}

/// Configuration for an upscaling engine instance
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineInstanceConfig {
    pub instance_id: Uuid,
    pub plugin_id: Uuid,
    pub engine_name: String,
    pub engine_config: String, // JSON string, since bincode cannot serde_json::Value
}

/// Information about an engine instance
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineInstance {
    pub config: EngineInstanceConfig,
    pub uptime_ms: u64,
}

/// Parameters for a preloading operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreloadParams {
    pub width: u32,
    pub height: u32,
    pub channels: u32,
}

/// Parameters for an upscaling operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpscaleParams {
    pub scale: u32,
    pub input_width: u32,
    pub input_height: u32,
    pub channels: u32,
    pub input_color_format: ColorFormat,
    pub desired_color_format: ColorFormat,
    pub input_data: Vec<u8>,
}

/// Result of an upscaling operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpscaleResult {
    pub processing_time_us: u64,
    pub output_width: u32,
    pub output_height: u32,
    pub channels: u32,
    pub output_color_format: ColorFormat,
    pub output_data: Vec<u8>,
}

/// Status information about the plugin host process
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginHostStatus {
    pub process_id: u32,
    pub uptime_ms: u64,
    pub total_operations: u64,
    pub successful_operations: u64,
    pub failed_operations: u64,
    pub memory_usage_bytes: u64,
    pub loaded_plugins: Vec<PluginInfo>,
}

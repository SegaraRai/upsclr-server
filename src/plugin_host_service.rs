use crate::models::{
    EngineConfigValidationResult, EngineInstance, EngineInstanceConfig, PluginHostError,
    PluginHostStatus, PluginInfo, PreloadParams, UpscaleParams, UpscaleResult,
};
use std::path::PathBuf;
use uuid::Uuid;

#[tarpc::service]
pub trait PluginHostService {
    /// Get the current process ID of the plugin host
    async fn get_process_id() -> u32;

    /// Get detailed status information about the plugin host
    async fn get_status() -> PluginHostStatus;

    /// Exit the plugin host process gracefully
    /// The main process will then restart the plugin host and restore all states
    async fn exit() -> ();

    /// Load plugin from the specified path and return its information or error
    async fn load_plugin(plugin_id: Uuid, path: PathBuf) -> Result<PluginInfo, PluginHostError>;

    /// Get information about a loaded plugin
    async fn get_plugin_info(plugin_id: Uuid) -> Result<PluginInfo, PluginHostError>;

    /// Validates the engine configuration for the specified engine
    async fn validate_engine_config(
        plugin_id: Uuid,
        engine_name: String,
        config: String,
    ) -> Result<EngineConfigValidationResult, PluginHostError>;

    /// Create a new engine instance with the specified configuration
    async fn create_engine_instance(
        config: EngineInstanceConfig,
    ) -> Result<EngineInstance, PluginHostError>;

    /// Destroy an existing engine instance
    async fn destroy_engine_instance(instance_id: Uuid) -> Result<(), PluginHostError>;

    /// Recreate an engine instance
    async fn recreate_engine_instance(
        id: Uuid,
        config: Option<EngineInstanceConfig>,
    ) -> Result<EngineInstance, PluginHostError>;

    /// Run preloading operation in the specified engine instance
    /// This prepares the engine for upscaling images of a specific scale
    async fn preload(id: Uuid, params: PreloadParams) -> Result<(), PluginHostError>;

    /// Run upscaling operation in the specified engine instance
    async fn upscale(id: Uuid, params: UpscaleParams) -> Result<UpscaleResult, PluginHostError>;

    /// Restore engine instances from saved plugin info
    /// Used after plugin host respawn to recreate the previous state
    async fn restore(saved_plugin_info: Option<PluginInfo>) -> Result<PluginInfo, PluginHostError>;
}

// Implementation of the plugin host service
// This handles a single plugin and its engine instances

use crate::models::{
    EngineConfigValidationResult, EngineInstance, EngineInstanceConfig, PluginHostError,
    PluginHostStatus, PluginInfo, PreloadParams, UpscaleParams, UpscaleResult,
};
use crate::plugin_host::instance_manager::InstanceManager;
use crate::plugin_host::plugin::LoadedPlugin;
use crate::plugin_host_service::PluginHostService;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tarpc::context::Context;
use tracing::{debug, error, info, trace};
use uuid::Uuid;

// Structure to hold the plugin host service state
#[derive(Clone)]
pub struct PluginHostServiceImpl {
    // Start time of the plugin host process
    start_time: Instant,
    // The loaded plugin
    plugin: Arc<RwLock<Option<Arc<LoadedPlugin>>>>,
    // Instance manager, which is dependent on the plugin
    instance_manager: Arc<RwLock<Option<Arc<InstanceManager>>>>,
    // Operation statistics
    total_operations: Arc<RwLock<u64>>,
    successful_operations: Arc<RwLock<u64>>,
    failed_operations: Arc<RwLock<u64>>,
}

impl Default for PluginHostServiceImpl {
    fn default() -> Self {
        Self {
            start_time: Instant::now(),
            plugin: Arc::new(RwLock::new(None)),
            instance_manager: Arc::new(RwLock::new(None)),
            total_operations: Arc::new(RwLock::new(0)),
            successful_operations: Arc::new(RwLock::new(0)),
            failed_operations: Arc::new(RwLock::new(0)),
        }
    }
}

impl PluginHostServiceImpl {
    // Create a new plugin host service
    pub fn new() -> Self {
        Self::default()
    }

    // Helper to track operation statistics
    fn track_operation<T, E>(&self, result: &Result<T, E>) {
        let mut total = self.total_operations.write().unwrap();
        *total += 1;

        match result {
            Ok(_) => {
                let mut success = self.successful_operations.write().unwrap();
                *success += 1;
            }
            Err(_) => {
                let mut failed = self.failed_operations.write().unwrap();
                *failed += 1;
            }
        }
    }
}

impl PluginHostService for PluginHostServiceImpl {
    // Get the current process ID of the plugin host
    async fn get_process_id(self, _: Context) -> u32 {
        std::process::id()
    }

    // Get detailed status information about the plugin host
    async fn get_status(self, _: Context) -> PluginHostStatus {
        let plugin = self.plugin.read().unwrap();
        let loaded_plugins = if let Some(plugin) = plugin.as_ref() {
            let mut plugin_info = plugin.plugin_info.clone();
            // Update the engine instances list
            if let Some(instance_manager) = self.instance_manager.read().unwrap().as_ref() {
                plugin_info.engine_instances = instance_manager.get_all_instances();
            }
            vec![plugin_info]
        } else {
            Vec::new()
        };

        PluginHostStatus {
            process_id: std::process::id(),
            uptime_ms: self.start_time.elapsed().as_millis() as u64,
            total_operations: *self.total_operations.read().unwrap(),
            successful_operations: *self.successful_operations.read().unwrap(),
            failed_operations: *self.failed_operations.read().unwrap(),
            memory_usage_bytes: 0, // Not implemented yet, would require platform-specific code
            loaded_plugins,
        }
    }

    // Exit the plugin host process gracefully
    async fn exit(self, _: Context) {
        info!("Received exit request, shutting down plugin host...");

        // First, clean up any engine instances
        if let Some(instance_manager) = self.instance_manager.read().unwrap().as_ref() {
            instance_manager.cleanup();
        }

        // Plugin library will be dropped when the process exits

        // Schedule a task to exit the process shortly
        tokio::spawn(async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            std::process::exit(0);
        });
    }

    // Load plugin from the specified path and return its information or error
    async fn load_plugin(
        self,
        _: Context,
        plugin_id: Uuid,
        path: PathBuf,
    ) -> Result<PluginInfo, PluginHostError> {
        info!(
            "Loading plugin from path: {:?} with ID: {}",
            path, plugin_id
        );

        // Check if a plugin is already loaded
        if self.plugin.read().unwrap().is_some() {
            return Err(PluginHostError::PluginLoadError(
                "Plugin host already has a plugin loaded. Each plugin host can only load one plugin.".to_string()
            ));
        }

        // Try to load the plugin
        let load_result = LoadedPlugin::load(plugin_id, path);
        self.track_operation(&load_result);

        match load_result {
            Ok(loaded_plugin) => {
                let plugin_info = loaded_plugin.plugin_info.clone();

                // Create the instance manager
                let instance_manager =
                    Arc::new(InstanceManager::new(Arc::new(loaded_plugin.clone())));

                // Store the loaded plugin and instance manager
                *self.plugin.write().unwrap() = Some(Arc::new(loaded_plugin));
                *self.instance_manager.write().unwrap() = Some(instance_manager);

                info!("Successfully loaded plugin: {}", plugin_info.name);
                trace!("Plugin info: {:?}", plugin_info);

                Ok(plugin_info)
            }
            Err(err) => {
                error!("Failed to load plugin: {:?}", err);
                Err(err)
            }
        }
    }

    // Validates the engine configuration for the specified engine
    async fn validate_engine_config(
        self,
        _: Context,
        plugin_id: Uuid,
        engine_name: String,
        config: serde_json::Value,
    ) -> Result<EngineConfigValidationResult, PluginHostError> {
        info!(
            "Validating engine config for engine: {} in plugin: {}",
            engine_name, plugin_id
        );

        // Get the loaded plugin
        let plugin_guard = self.plugin.read().unwrap();
        let plugin = plugin_guard.as_ref().ok_or_else(|| {
            PluginHostError::PluginNotFound("No plugin loaded in this plugin host".to_string())
        })?;

        // Check if this is the correct plugin ID
        if plugin.id != plugin_id {
            return Err(PluginHostError::PluginNotFound(format!(
                "Plugin ID mismatch. Expected: {}, but this host has: {}",
                plugin_id, plugin.id
            )));
        }

        // Call the plugin to validate the configuration
        let result = plugin.validate_engine_config(&engine_name, config);
        self.track_operation(&result);

        result
    }

    // Create a new engine instance with the specified configuration
    async fn create_engine_instance(
        self,
        _: Context,
        config: EngineInstanceConfig,
    ) -> Result<EngineInstance, PluginHostError> {
        info!("Creating engine instance with ID: {}", config.instance_id);

        // Get the loaded plugin and check plugin ID
        let plugin_guard = self.plugin.read().unwrap();
        let plugin = plugin_guard.as_ref().ok_or_else(|| {
            PluginHostError::PluginNotFound("No plugin loaded in this plugin host".to_string())
        })?;

        if plugin.id != config.plugin_id {
            return Err(PluginHostError::PluginNotFound(format!(
                "Plugin ID mismatch. Expected: {}, but this host has: {}",
                config.plugin_id, plugin.id
            )));
        }

        // Get the instance manager
        let instance_manager_guard = self.instance_manager.read().unwrap();
        let instance_manager = instance_manager_guard.as_ref().ok_or_else(|| {
            PluginHostError::InternalError("Instance manager not initialized".to_string())
        })?;

        // Create the engine instance
        let result = instance_manager.create_instance(config);
        self.track_operation(&result);

        result
    }

    // Destroy an existing engine instance
    async fn destroy_engine_instance(
        self,
        _: Context,
        instance_id: Uuid,
    ) -> Result<(), PluginHostError> {
        info!("Destroying engine instance with ID: {}", instance_id);

        // Get the instance manager
        let instance_manager_guard = self.instance_manager.read().unwrap();
        let instance_manager = instance_manager_guard.as_ref().ok_or_else(|| {
            PluginHostError::InternalError("Instance manager not initialized".to_string())
        })?;

        // Destroy the instance
        let result = instance_manager.destroy_instance(&instance_id);
        self.track_operation(&result);

        result
    }

    // Recreate an engine instance
    async fn recreate_engine_instance(
        self,
        _: Context,
        id: Uuid,
        config: Option<EngineInstanceConfig>,
    ) -> Result<EngineInstance, PluginHostError> {
        info!("Recreating engine instance with ID: {}", id);

        // Get the instance manager
        let instance_manager_guard = self.instance_manager.read().unwrap();
        let instance_manager = instance_manager_guard.as_ref().ok_or_else(|| {
            PluginHostError::InternalError("Instance manager not initialized".to_string())
        })?;

        // Recreate the instance
        let result = instance_manager.recreate_instance(&id, config);
        self.track_operation(&result);

        result
    }

    // Run preloading operation in the specified engine instance
    async fn preload(
        self,
        _: Context,
        id: Uuid,
        params: PreloadParams,
    ) -> Result<(), PluginHostError> {
        debug!(
            "Preloading engine instance: {} with params: {:?}",
            id, params
        );

        // Get the instance manager
        let instance_manager_guard = self.instance_manager.read().unwrap();
        let instance_manager = instance_manager_guard.as_ref().ok_or_else(|| {
            PluginHostError::InternalError("Instance manager not initialized".to_string())
        })?;

        // Run preload
        let result = instance_manager.preload(&id, params);
        self.track_operation(&result);

        result
    }

    // Run upscaling operation in the specified engine instance
    async fn upscale(
        self,
        _: Context,
        id: Uuid,
        params: UpscaleParams,
    ) -> Result<UpscaleResult, PluginHostError> {
        debug!("Upscaling image with engine instance: {}", id);

        // Get the instance manager
        let instance_manager_guard = self.instance_manager.read().unwrap();
        let instance_manager = instance_manager_guard.as_ref().ok_or_else(|| {
            PluginHostError::InternalError("Instance manager not initialized".to_string())
        })?;

        // Run upscale
        let result = instance_manager.upscale(&id, params);
        self.track_operation(&result);

        result
    }
}

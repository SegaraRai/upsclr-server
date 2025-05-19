// Manages plugin host processes

use crate::main_process::plugin_host_client::PluginHostConnection;
use crate::models::{
    EngineConfigValidationResult, EngineInstance, EngineInstanceConfig, PluginHostError,
    PluginHostStatus, PluginInfo, PreloadParams, UpscaleParams, UpscaleResult,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{debug, error, info};
use uuid::Uuid;

// Manages all plugin hosts
pub struct PluginManager {
    // Path to the server executable
    executable_path: PathBuf,
    // Map of plugin ID to plugin host connection
    plugin_hosts: RwLock<HashMap<Uuid, Arc<RwLock<PluginHostConnection>>>>,
}

impl PluginManager {
    // Create a new plugin manager
    pub fn new(executable_path: PathBuf) -> Self {
        Self {
            executable_path,
            plugin_hosts: RwLock::new(HashMap::new()),
        }
    }

    // Load a plugin and start a plugin host process
    pub async fn load_plugin(&self, plugin_path: PathBuf) -> Result<PluginInfo, PluginHostError> {
        // Generate a new UUID for the plugin
        let plugin_id = Uuid::new_v4();

        // Start the plugin host process
        let mut connection = PluginHostConnection::start_plugin_host(
            self.executable_path.clone(),
            plugin_id,
            plugin_path.clone(),
        )
        .await
        .map_err(|e| {
            PluginHostError::PluginLoadError(format!("Failed to start plugin host: {}", e))
        })?;

        // Perform some basic RPC calls to make debug easier
        let pid = connection.get_process_id().await.map_err(|e| {
            PluginHostError::PluginLoadError(format!(
                "Failed to get plugin host process ID: {:?}",
                e
            ))
        })?;
        debug!("Plugin host process ID: {}", pid);

        let status = connection.get_status().await.map_err(|e| {
            PluginHostError::PluginLoadError(format!("Failed to get plugin host status: {:?}", e))
        })?;
        debug!("Plugin host status: {:?}", status);

        // Load the plugin
        let plugin_info = connection.load_plugin().await?;

        // Store the connection
        let connection_arc = Arc::new(RwLock::new(connection));
        self.plugin_hosts
            .write()
            .await
            .insert(plugin_id, connection_arc.clone());

        Ok(plugin_info)
    }

    // Get all loaded plugins
    pub async fn get_all_plugins(&self) -> Vec<PluginInfo> {
        let mut plugin_infos = Vec::new();

        for connection_arc in self.plugin_hosts.read().await.values() {
            let connection = connection_arc.read().await;
            if let Ok(status) = connection.get_status().await {
                plugin_infos.extend(status.loaded_plugins);
            }
        }

        plugin_infos
    }

    // Find a plugin by ID
    pub async fn find_plugin_host(
        &self,
        plugin_id: &Uuid,
    ) -> Option<Arc<RwLock<PluginHostConnection>>> {
        self.plugin_hosts.read().await.get(plugin_id).cloned()
    }

    // Validate engine configuration
    pub async fn validate_engine_config(
        &self,
        plugin_id: Uuid,
        engine_name: String,
        config: serde_json::Value,
    ) -> Result<EngineConfigValidationResult, PluginHostError> {
        // Find the plugin host
        let plugin_host = self
            .find_plugin_host(&plugin_id)
            .await
            .ok_or_else(|| PluginHostError::PluginNotFound(plugin_id.to_string()))?;

        // Call the plugin host
        let connection = plugin_host.read().await;
        connection.validate_engine_config(engine_name, config).await
    }

    // Create an engine instance
    pub async fn create_engine_instance(
        &self,
        config: EngineInstanceConfig,
    ) -> Result<EngineInstance, PluginHostError> {
        // Find the plugin host
        let plugin_host = self
            .find_plugin_host(&config.plugin_id)
            .await
            .ok_or_else(|| PluginHostError::PluginNotFound(config.plugin_id.to_string()))?;

        // Call the plugin host - use write lock for &mut self method
        let mut connection = plugin_host.write().await;
        connection.create_engine_instance(config).await
    }

    // Destroy an engine instance
    pub async fn destroy_engine_instance(
        &self,
        plugin_id: Uuid,
        instance_id: Uuid,
    ) -> Result<(), PluginHostError> {
        // Find the plugin host
        let plugin_host = self
            .find_plugin_host(&plugin_id)
            .await
            .ok_or_else(|| PluginHostError::PluginNotFound(plugin_id.to_string()))?;

        // Call the plugin host - use write lock for &mut self method
        let mut connection = plugin_host.write().await;
        connection.destroy_engine_instance(instance_id).await
    }

    // Recreate an engine instance
    pub async fn recreate_engine_instance(
        &self,
        plugin_id: Uuid,
        instance_id: Uuid,
        config: Option<EngineInstanceConfig>,
    ) -> Result<EngineInstance, PluginHostError> {
        // Find the plugin host
        let plugin_host = self
            .find_plugin_host(&plugin_id)
            .await
            .ok_or_else(|| PluginHostError::PluginNotFound(plugin_id.to_string()))?;

        // Call the plugin host - use write lock for &mut self method
        let mut connection = plugin_host.write().await;
        connection
            .recreate_engine_instance(instance_id, config)
            .await
    }

    // Run preload operation
    pub async fn preload(
        &self,
        plugin_id: Uuid,
        instance_id: Uuid,
        params: PreloadParams,
    ) -> Result<(), PluginHostError> {
        // Find the plugin host
        let plugin_host = self
            .find_plugin_host(&plugin_id)
            .await
            .ok_or_else(|| PluginHostError::PluginNotFound(plugin_id.to_string()))?;

        // Call the plugin host
        let connection = plugin_host.read().await;
        connection.preload(instance_id, params).await
    }

    // Run upscale operation
    pub async fn upscale(
        &self,
        plugin_id: Uuid,
        instance_id: Uuid,
        params: UpscaleParams,
    ) -> Result<UpscaleResult, PluginHostError> {
        // Find the plugin host
        let plugin_host = self
            .find_plugin_host(&plugin_id)
            .await
            .ok_or_else(|| PluginHostError::PluginNotFound(plugin_id.to_string()))?;

        // Call the plugin host
        let connection = plugin_host.read().await;
        connection.upscale(instance_id, params).await
    }

    // Get status of a plugin host
    pub async fn get_plugin_host_status(
        &self,
        plugin_id: &Uuid,
    ) -> Result<PluginHostStatus, PluginHostError> {
        // Find the plugin host
        let plugin_host = self
            .find_plugin_host(plugin_id)
            .await
            .ok_or_else(|| PluginHostError::PluginNotFound(plugin_id.to_string()))?;

        // Call the plugin host
        let connection = plugin_host.read().await;
        connection.get_status().await
    }

    // Exit all plugin hosts gracefully
    pub async fn exit_all(&self) {
        let plugin_hosts = self.plugin_hosts.read().await;

        // Close concurrently to avoid blocking
        let exit_futures: Vec<_> = plugin_hosts
            .iter()
            .map(async |(plugin_id, connection_arc)| {
                // First send exit command with a read lock
                {
                    let connection = connection_arc.read().await;
                    info!("Exiting plugin host for plugin: {}", plugin_id);

                    // Send RPC command
                    let _ = connection.exit().await.inspect_err(|err| {
                        error!("Error exiting plugin host: {:?}", err);
                    });
                }

                // Then, wait for the exit to complete with a write lock
                {
                    let mut connection = connection_arc.write().await;
                    let _ = connection.wait_for_exit().await.inspect_err(|err| {
                        error!("Error waiting for plugin host exit: {:?}", err);
                    });
                }
            })
            .collect();

        // Wait for all exits to complete
        futures::future::join_all(exit_futures).await;
    }

    // Kill all plugin hosts forcefully
    pub async fn kill_all(&self, timeout: Duration) {
        // Kill sequentially to avoid overwhelming the system
        for (plugin_id, connection_arc) in self.plugin_hosts.read().await.iter() {
            let mut connection = connection_arc.write().await;
            info!("Killing plugin host for plugin: {}", plugin_id);
            match tokio::time::timeout(timeout, connection.kill()).await {
                Ok(Ok(())) => {
                    info!("Successfully killed plugin host for plugin: {}", plugin_id);
                }
                Ok(Err(err)) => {
                    error!(
                        "Error killing plugin host for plugin {}: {}",
                        plugin_id, err
                    );
                }
                Err(err) => {
                    error!(
                        "Timeout while killing plugin host for plugin {}: {}",
                        plugin_id, err
                    );
                }
            }
        }
    }

    // Scan a directory for plugins and load them
    pub async fn scan_and_load_plugins(
        &self,
        plugins_dir: &Path,
    ) -> Result<Vec<PluginInfo>, PluginHostError> {
        let mut loaded_plugins = Vec::new();

        // Check if directory exists
        if !plugins_dir.is_dir() {
            return Err(PluginHostError::PluginLoadError(format!(
                "Plugin directory not found: {:?}",
                plugins_dir
            )));
        }

        // Read directory
        let entries = std::fs::read_dir(plugins_dir).map_err(|e| {
            PluginHostError::PluginLoadError(format!("Failed to read plugin directory: {}", e))
        })?;

        // Load each plugin
        for entry in entries {
            let entry = entry.map_err(|e| {
                PluginHostError::PluginLoadError(format!("Failed to read directory entry: {}", e))
            })?;

            let path = entry.path();

            // Skip if not a file
            if !path.is_file() {
                continue;
            }

            // Check if it's a plugin file (based on extension and naming convention)
            let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            let extension = path.extension().and_then(|s| s.to_str()).unwrap_or("");

            if file_name.starts_with("upsclr-plugin-")
                && matches!(extension, "dll" | "so" | "dylib")
            {
                info!("Found plugin: {:?}", path);
                match self.load_plugin(path.clone()).await {
                    Ok(plugin_info) => {
                        info!("Successfully loaded plugin: {}", plugin_info.name);
                        loaded_plugins.push(plugin_info);
                    }
                    Err(e) => {
                        error!("Failed to load plugin {:?}: {:?}", path, e);
                    }
                }
            }
        }

        Ok(loaded_plugins)
    }
}

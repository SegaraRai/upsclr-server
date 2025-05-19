// Client for communicating with a plugin host process

use crate::ipc::BootstrapHost;
use crate::models::{
    EngineConfigValidationResult, EngineInstance, EngineInstanceConfig, PluginHostError,
    PluginHostStatus, PluginInfo, PreloadParams, UpscaleParams, UpscaleResult,
};
use crate::plugin_host_service::PluginHostServiceClient;
use std::path::PathBuf;
use std::time::Duration;
use tarpc::context;
use tokio::process::{Child, Command};
use tracing::info;
use uuid::Uuid;

pub enum PluginHostRespawnMode {
    /// Restart the plugin host process if it crashes
    RestartOnCrash,
    /// Do not restart the plugin host process
    /// This is meant to be a temporary suspension, e.g. when plugin host crashes repeatedly
    Suspended,
    /// Do not restart the plugin host process
    /// This is meant to be used when the application is shutting down
    NoRestart,
}

pub struct PluginHostProcess {
    // Dispatcher
    _dispatcher_task: tokio::task::JoinHandle<()>,
    // The actual RPC client
    client: PluginHostServiceClient,
    // The plugin host process
    process: Child,
}

// Represents a connection to a plugin host process
pub struct PluginHostConnection {
    // The plugin ID for the loaded plugin
    plugin_id: Uuid,
    // Path to the plugin file
    plugin_path: PathBuf,
    // Latest plugin information with its engine instances
    plugin_info: Option<PluginInfo>,
    // Whether the plugin has been loaded successfully
    plugin_loaded: bool,
    // Respawn mode for the plugin host process
    respawn_mode: PluginHostRespawnMode,
    // Plugin host process
    process: Option<PluginHostProcess>,
}

impl PluginHostConnection {
    // Start a new plugin host process and connect to it
    pub async fn start_plugin_host(
        executable_path: PathBuf,
        plugin_id: Uuid,
        plugin_path: PathBuf,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        // Create bootstrap server to exchange transport information
        let (bootstrap_host, bootstrap_name) = BootstrapHost::new()?;
        info!("Created bootstrap server: {}", bootstrap_name);

        // Start the plugin host process
        let process = Command::new(&executable_path)
            .arg("--mode=plugin_host")
            .arg(format!("--ipc={}", bootstrap_name))
            .spawn()?;

        let process_id = process.id().unwrap_or(0);
        info!("Started plugin host process: {}", process_id);

        // Wait for the plugin host process to connect
        info!("Waiting for plugin host process to connect...");
        let (transport, _connection) = bootstrap_host.accept(Duration::from_secs(10)).await?;
        info!("Plugin host process connected");

        // Create the RPC client
        let new_client = PluginHostServiceClient::new(tarpc::client::Config::default(), transport);
        let client = new_client.client;
        let dispatcher_task = tokio::spawn(async move {
            let _ = new_client.dispatch.await.inspect_err(|err| {
                tracing::error!("Error in dispatcher: {}", err);
            });
            tracing::info!("Dispatcher task finished");
        });

        // Create the process
        let plugin_host_process = PluginHostProcess {
            _dispatcher_task: dispatcher_task,
            client,
            process,
        };

        // Create the connection
        let connection = Self {
            plugin_id,
            plugin_path,
            plugin_info: None,
            plugin_loaded: false,
            respawn_mode: PluginHostRespawnMode::RestartOnCrash,
            process: Some(plugin_host_process),
        };

        Ok(connection)
    }

    // Load the plugin in the plugin host process
    pub async fn load_plugin(&mut self) -> Result<PluginInfo, PluginHostError> {
        if self.plugin_loaded {
            return Err(PluginHostError::PluginLoadError(
                "Plugin already loaded".to_string(),
            ));
        }

        // Create a context with timeout
        let mut ctx = context::current();
        ctx.deadline = std::time::Instant::now() + Duration::from_secs(30);

        // Load the plugin
        if let Some(process) = &mut self.process {
            let result = process
                .client
                .load_plugin(ctx, self.plugin_id, self.plugin_path.clone())
                .await
                .map_err(|e| PluginHostError::PluginLoadError(format!("RPC error: {}", e)))??;

            self.plugin_loaded = true;

            // Store the plugin info
            self.plugin_info = Some(result.clone());

            Ok(result)
        } else {
            Err(PluginHostError::InternalError(
                "Plugin host process not available".to_string(),
            ))
        }
    }

    // Get the process ID of the plugin host
    pub async fn get_process_id(&self) -> Result<u32, PluginHostError> {
        if let Some(process) = &self.process {
            let ctx = context::current();
            process
                .client
                .get_process_id(ctx)
                .await
                .map_err(|e| PluginHostError::InternalError(format!("RPC error: {}", e)))
        } else {
            Err(PluginHostError::InternalError(
                "Plugin host process not available".to_string(),
            ))
        }
    }

    // Get the status of the plugin host
    pub async fn get_status(&self) -> Result<PluginHostStatus, PluginHostError> {
        if let Some(process) = &self.process {
            let ctx = context::current();
            process
                .client
                .get_status(ctx)
                .await
                .map_err(|e| PluginHostError::InternalError(format!("RPC error: {}", e)))
        } else {
            Err(PluginHostError::InternalError(
                "Plugin host process not available".to_string(),
            ))
        }
    }

    // Exit the plugin host process
    pub async fn exit(&self) -> Result<(), PluginHostError> {
        if let Some(process) = &self.process {
            let ctx = context::current();
            process
                .client
                .exit(ctx)
                .await
                .map_err(|e| PluginHostError::InternalError(format!("RPC error: {}", e)))
        } else {
            Err(PluginHostError::InternalError(
                "Plugin host process not available".to_string(),
            ))
        }
    }

    // Wait for the plugin host process to exit gracefully
    pub async fn wait_for_exit(&mut self) -> Result<(), std::io::Error> {
        if let Some(process) = self.process.take() {
            let mut child_process = process.process;
            child_process.wait().await?;
            // Don't put the process back since it's now exited
        }
        Ok(())
    }

    // Kill the plugin host process forcefully
    pub async fn kill(&mut self) -> Result<(), std::io::Error> {
        if let Some(process) = self.process.take() {
            let mut child_process = process.process;
            child_process.kill().await?;
            // Don't put the process back since it's now killed
        }
        Ok(())
    }

    // Validate engine configuration
    pub async fn validate_engine_config(
        &self,
        engine_name: String,
        config: serde_json::Value,
    ) -> Result<EngineConfigValidationResult, PluginHostError> {
        if !self.plugin_loaded {
            return Err(PluginHostError::PluginNotFound(
                "Plugin not loaded".to_string(),
            ));
        }

        let config_str = serde_json::to_string(&config).map_err(|err| {
            PluginHostError::InvalidParameter(format!("Failed to serialize config: {}", err))
        })?;

        if let Some(process) = &self.process {
            let ctx = context::current();
            process
                .client
                .validate_engine_config(ctx, self.plugin_id, engine_name, config_str)
                .await
                .map_err(|e| PluginHostError::InternalError(format!("RPC error: {}", e)))?
        } else {
            Err(PluginHostError::InternalError(
                "Plugin host process not available".to_string(),
            ))
        }
    }

    // Create an engine instance
    pub async fn create_engine_instance(
        &mut self,
        config: EngineInstanceConfig,
    ) -> Result<EngineInstance, PluginHostError> {
        if !self.plugin_loaded {
            return Err(PluginHostError::PluginNotFound(
                "Plugin not loaded".to_string(),
            ));
        }

        if let Some(process) = &mut self.process {
            let ctx = context::current();
            let result = process
                .client
                .create_engine_instance(ctx, config)
                .await
                .map_err(|e| PluginHostError::InternalError(format!("RPC error: {}", e)))??;

            // Update plugin info after mutation
            self.update_plugin_info().await?;

            Ok(result)
        } else {
            Err(PluginHostError::InternalError(
                "Plugin host process not available".to_string(),
            ))
        }
    }

    // Destroy an engine instance
    pub async fn destroy_engine_instance(
        &mut self,
        instance_id: Uuid,
    ) -> Result<(), PluginHostError> {
        if !self.plugin_loaded {
            return Err(PluginHostError::PluginNotFound(
                "Plugin not loaded".to_string(),
            ));
        }

        if let Some(process) = &mut self.process {
            let ctx = context::current();
            process
                .client
                .destroy_engine_instance(ctx, instance_id)
                .await
                .map_err(|e| PluginHostError::InternalError(format!("RPC error: {}", e)))??;

            // Update plugin info after mutation
            self.update_plugin_info().await?;

            Ok(())
        } else {
            Err(PluginHostError::InternalError(
                "Plugin host process not available".to_string(),
            ))
        }
    }

    // Recreate an engine instance
    pub async fn recreate_engine_instance(
        &mut self,
        id: Uuid,
        config: Option<EngineInstanceConfig>,
    ) -> Result<EngineInstance, PluginHostError> {
        if !self.plugin_loaded {
            return Err(PluginHostError::PluginNotFound(
                "Plugin not loaded".to_string(),
            ));
        }

        if let Some(process) = &mut self.process {
            let ctx = context::current();
            let result = process
                .client
                .recreate_engine_instance(ctx, id, config)
                .await
                .map_err(|e| PluginHostError::InternalError(format!("RPC error: {}", e)))??;

            // Update plugin info after mutation
            self.update_plugin_info().await?;

            Ok(result)
        } else {
            Err(PluginHostError::InternalError(
                "Plugin host process not available".to_string(),
            ))
        }
    }

    // Preload an engine instance
    pub async fn preload(&self, id: Uuid, params: PreloadParams) -> Result<(), PluginHostError> {
        if !self.plugin_loaded {
            return Err(PluginHostError::PluginNotFound(
                "Plugin not loaded".to_string(),
            ));
        }

        if let Some(process) = &self.process {
            let ctx = context::current();
            process
                .client
                .preload(ctx, id, params)
                .await
                .map_err(|e| PluginHostError::InternalError(format!("RPC error: {}", e)))?
        } else {
            Err(PluginHostError::InternalError(
                "Plugin host process not available".to_string(),
            ))
        }
    }

    // Upscale an image
    pub async fn upscale(
        &self,
        id: Uuid,
        params: UpscaleParams,
    ) -> Result<UpscaleResult, PluginHostError> {
        if !self.plugin_loaded {
            return Err(PluginHostError::PluginNotFound(
                "Plugin not loaded".to_string(),
            ));
        }

        if let Some(process) = &self.process {
            let ctx = context::current();
            process
                .client
                .upscale(ctx, id, params)
                .await
                .map_err(|e| PluginHostError::InternalError(format!("RPC error: {}", e)))?
        } else {
            Err(PluginHostError::InternalError(
                "Plugin host process not available".to_string(),
            ))
        }
    }

    // Update plugin info from the plugin host
    async fn update_plugin_info(&mut self) -> Result<(), PluginHostError> {
        if !self.plugin_loaded {
            return Ok(());
        }

        if let Some(process) = &mut self.process {
            let ctx = context::current();
            let plugin_info = process
                .client
                .get_plugin_info(ctx, self.plugin_id)
                .await
                .map_err(|e| PluginHostError::InternalError(format!("RPC error: {}", e)))??;

            self.plugin_info = Some(plugin_info);
            Ok(())
        } else {
            Err(PluginHostError::InternalError(
                "Plugin host process not available".to_string(),
            ))
        }
    }

    // Get the current plugin info
    pub async fn get_plugin_info(
        &mut self,
        no_cache: bool,
    ) -> Result<Option<PluginInfo>, PluginHostError> {
        if no_cache && !self.plugin_loaded {
            return Ok(None);
        }

        // Update from the plugin host to get the latest info
        if !no_cache {
            self.update_plugin_info().await?;
        }

        // Return the stored info
        Ok(self.plugin_info.clone())
    }
}

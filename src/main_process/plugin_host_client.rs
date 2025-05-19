// Client for communicating with a plugin host process

use crate::ipc::BootstrapHost;
use crate::models::{
    EngineConfigValidationResult, EngineInstance, EngineInstanceConfig, PluginHostError,
    PluginHostStatus, PluginInfo, PreloadParams, UpscaleParams, UpscaleResult,
};
use crate::plugin_host_service::PluginHostServiceClient;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tarpc::context;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
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

// Represents a connection to a plugin host process
pub struct PluginHostConnection {
    // Dispatcher
    _dispatcher_task: tokio::task::JoinHandle<()>,
    // The actual RPC client
    client: PluginHostServiceClient,
    // The plugin host process
    process: Arc<Mutex<Option<Child>>>,
    // The plugin ID for the loaded plugin
    plugin_id: Uuid,
    // Path to the plugin file
    plugin_path: PathBuf,
    // Whether the plugin has been loaded successfully
    plugin_loaded: bool,
    // Latest plugin information with its engine instances
    plugin_info: Arc<Mutex<Option<PluginInfo>>>,
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

        // Create the connection
        let connection = Self {
            _dispatcher_task: dispatcher_task,
            client,
            process: Arc::new(Mutex::new(Some(process))),
            plugin_id,
            plugin_path,
            plugin_loaded: false,
            plugin_info: Arc::new(Mutex::new(None)),
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
        let result = self
            .client
            .load_plugin(ctx, self.plugin_id, self.plugin_path.clone())
            .await
            .map_err(|e| PluginHostError::PluginLoadError(format!("RPC error: {}", e)))??;

        self.plugin_loaded = true;

        // Store the plugin info
        let mut info_guard = self.plugin_info.lock().await;
        *info_guard = Some(result.clone());

        Ok(result)
    }

    // Get the process ID of the plugin host
    pub async fn get_process_id(&self) -> Result<u32, PluginHostError> {
        let ctx = context::current();
        self.client
            .get_process_id(ctx)
            .await
            .map_err(|e| PluginHostError::InternalError(format!("RPC error: {}", e)))
    }

    // Get the status of the plugin host
    pub async fn get_status(&self) -> Result<PluginHostStatus, PluginHostError> {
        let ctx = context::current();
        self.client
            .get_status(ctx)
            .await
            .map_err(|e| PluginHostError::InternalError(format!("RPC error: {}", e)))
    }

    // Exit the plugin host process
    pub async fn exit(&self) -> Result<(), PluginHostError> {
        let ctx = context::current();
        self.client
            .exit(ctx)
            .await
            .map_err(|e| PluginHostError::InternalError(format!("RPC error: {}", e)))
    }

    // Wait for the plugin host process to exit gracefully
    pub async fn wait_for_exit(&self) -> Result<(), std::io::Error> {
        let mut process_guard = self.process.lock().await;
        if let Some(mut process) = process_guard.take() {
            process.wait().await?;
        }
        Ok(())
    }

    // Kill the plugin host process forcefully
    pub async fn kill(&self) -> Result<(), std::io::Error> {
        let mut process_guard = self.process.lock().await;
        if let Some(mut process) = process_guard.take() {
            process.kill().await?;
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

        let ctx = context::current();
        self.client
            .validate_engine_config(ctx, self.plugin_id, engine_name, config_str)
            .await
            .map_err(|e| PluginHostError::InternalError(format!("RPC error: {}", e)))?
    }

    // Create an engine instance
    pub async fn create_engine_instance(
        &self,
        config: EngineInstanceConfig,
    ) -> Result<EngineInstance, PluginHostError> {
        if !self.plugin_loaded {
            return Err(PluginHostError::PluginNotFound(
                "Plugin not loaded".to_string(),
            ));
        }

        let ctx = context::current();
        let result = self
            .client
            .create_engine_instance(ctx, config)
            .await
            .map_err(|e| PluginHostError::InternalError(format!("RPC error: {}", e)))??;

        // Update plugin info after mutation
        self.update_plugin_info().await?;

        Ok(result)
    }

    // Destroy an engine instance
    pub async fn destroy_engine_instance(&self, instance_id: Uuid) -> Result<(), PluginHostError> {
        if !self.plugin_loaded {
            return Err(PluginHostError::PluginNotFound(
                "Plugin not loaded".to_string(),
            ));
        }

        let ctx = context::current();
        self.client
            .destroy_engine_instance(ctx, instance_id)
            .await
            .map_err(|e| PluginHostError::InternalError(format!("RPC error: {}", e)))??;

        // Update plugin info after mutation
        self.update_plugin_info().await?;

        Ok(())
    }

    // Recreate an engine instance
    pub async fn recreate_engine_instance(
        &self,
        id: Uuid,
        config: Option<EngineInstanceConfig>,
    ) -> Result<EngineInstance, PluginHostError> {
        if !self.plugin_loaded {
            return Err(PluginHostError::PluginNotFound(
                "Plugin not loaded".to_string(),
            ));
        }

        let ctx = context::current();
        let result = self
            .client
            .recreate_engine_instance(ctx, id, config)
            .await
            .map_err(|e| PluginHostError::InternalError(format!("RPC error: {}", e)))??;

        // Update plugin info after mutation
        self.update_plugin_info().await?;

        Ok(result)
    }

    // Preload an engine instance
    pub async fn preload(&self, id: Uuid, params: PreloadParams) -> Result<(), PluginHostError> {
        if !self.plugin_loaded {
            return Err(PluginHostError::PluginNotFound(
                "Plugin not loaded".to_string(),
            ));
        }

        let ctx = context::current();
        self.client
            .preload(ctx, id, params)
            .await
            .map_err(|e| PluginHostError::InternalError(format!("RPC error: {}", e)))?
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

        let ctx = context::current();
        self.client
            .upscale(ctx, id, params)
            .await
            .map_err(|e| PluginHostError::InternalError(format!("RPC error: {}", e)))?
    }

    // Update plugin info from the plugin host
    async fn update_plugin_info(&self) -> Result<(), PluginHostError> {
        if !self.plugin_loaded {
            return Ok(());
        }

        let ctx = context::current();
        let plugin_info = self
            .client
            .get_plugin_info(ctx, self.plugin_id)
            .await
            .map_err(|e| PluginHostError::InternalError(format!("RPC error: {}", e)))??;

        let mut info_guard = self.plugin_info.lock().await;
        *info_guard = Some(plugin_info);

        Ok(())
    }

    // Get the current plugin info
    pub async fn get_plugin_info(&self) -> Result<Option<PluginInfo>, PluginHostError> {
        if !self.plugin_loaded {
            return Ok(None);
        }

        // Update from the plugin host to get the latest info
        self.update_plugin_info().await?;

        // Return the stored info
        let info_guard = self.plugin_info.lock().await;
        Ok(info_guard.clone())
    }
}

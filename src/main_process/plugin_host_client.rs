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

#[derive(Debug, Clone, Copy)]
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
    // Latest plugin information with its engine instances to restore
    plugin_info: Option<PluginInfo>,
}

// Represents a connection to a plugin host process
pub struct PluginHostConnection {
    // The plugin ID for the loaded plugin
    plugin_id: Uuid,
    // Path to the plugin file
    plugin_path: PathBuf,
    // Whether the plugin has been loaded successfully
    plugin_loaded: bool,
    // Respawn mode for the plugin host process
    respawn_mode: Arc<Mutex<PluginHostRespawnMode>>,
    // Plugin host process
    process: Option<Arc<Mutex<PluginHostProcess>>>,
    // Handle for the respawn task
    respawn_task: Option<tokio::task::JoinHandle<()>>,
    // Path to the executable
    executable_path: PathBuf,
}

impl PluginHostConnection {
    // Start a new plugin host process and connect to it
    pub async fn new(
        executable_path: PathBuf,
        plugin_id: Uuid,
        plugin_path: PathBuf,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        // Create the initial connection object
        let mut connection = Self {
            plugin_id,
            plugin_path,
            plugin_loaded: false,
            respawn_mode: Arc::new(Mutex::new(PluginHostRespawnMode::RestartOnCrash)),
            process: None,
            respawn_task: None,
            executable_path,
        };

        // Start the plugin host process
        let (process_arc, _bootstrap_name) = connection.start_plugin_host_process().await?;

        // Set up respawn task
        connection.process = Some(process_arc.clone());

        // Create the respawn task
        connection.create_respawn_task(process_arc);

        Ok(connection)
    }

    // Load the plugin in the plugin host process
    pub async fn load_plugin(&mut self) -> Result<PluginInfo, PluginHostError> {
        if self.plugin_loaded {
            return Err(PluginHostError::PluginLoadError(
                "Plugin already loaded".to_string(),
            ));
        }

        // Load the plugin
        if let Some(process_arc) = &self.process {
            let mut process_guard = process_arc.lock().await;

            // Create a context with timeout
            let mut ctx = context::current();
            ctx.deadline = std::time::Instant::now() + Duration::from_secs(30);

            let result = process_guard
                .client
                .load_plugin(ctx, self.plugin_id, self.plugin_path.clone())
                .await
                .map_err(|e| PluginHostError::PluginLoadError(format!("RPC error: {}", e)))??;

            // Store the plugin info
            process_guard.plugin_info = Some(result.clone());

            self.plugin_loaded = true;

            Ok(result)
        } else {
            Err(PluginHostError::InternalError(
                "Plugin host process not available".to_string(),
            ))
        }
    }

    // Get the process ID of the plugin host
    pub async fn get_process_id(&self) -> Result<u32, PluginHostError> {
        if let Some(process_arc) = &self.process {
            let process_guard = process_arc.lock().await;

            let ctx = context::current();
            process_guard
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
        if let Some(process_arc) = &self.process {
            let process_guard = process_arc.lock().await;

            let ctx = context::current();
            process_guard
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
        if let Some(process_arc) = &self.process {
            let process_guard = process_arc.lock().await;

            let ctx = context::current();
            process_guard
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
        if let Some(process_arc) = self.process.take() {
            // Cancel the respawn task first
            if let Some(task) = self.respawn_task.take() {
                task.abort();
            }

            // Wait for the process to exit
            let mut process_guard = process_arc.lock().await;
            process_guard.process.wait().await?;
            // Don't put the process back since it's now exited
        }
        Ok(())
    }

    // Kill the plugin host process forcefully
    pub async fn kill(&mut self) -> Result<(), std::io::Error> {
        if let Some(process_arc) = self.process.take() {
            // Cancel the respawn task first
            if let Some(task) = self.respawn_task.take() {
                task.abort();
            }

            // Kill the process
            let mut process_guard = process_arc.lock().await;
            process_guard.process.kill().await?;
            // Don't put the process back since it's now killed
        }
        Ok(())
    }

    // Set the respawn mode for the plugin host process
    // Make sure to call this before calling `exit` or `kill` on application shutdown
    pub async fn set_respawn_mode(
        &mut self,
        mode: PluginHostRespawnMode,
    ) -> Result<(), PluginHostError> {
        let mut respawn_mode = self.respawn_mode.lock().await;
        *respawn_mode = mode;
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

        if let Some(process_arc) = &self.process {
            let process_guard = process_arc.lock().await;

            let ctx = context::current();
            process_guard
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

        if let Some(process_arc) = &self.process {
            let result = {
                let process_guard = process_arc.lock().await;

                let ctx = context::current();
                process_guard
                    .client
                    .create_engine_instance(ctx, config)
                    .await
                    .map_err(|e| PluginHostError::InternalError(format!("RPC error: {}", e)))??
            };

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

        if let Some(process_arc) = &self.process {
            {
                let process_guard = process_arc.lock().await;

                let ctx = context::current();
                process_guard
                    .client
                    .destroy_engine_instance(ctx, instance_id)
                    .await
                    .map_err(|e| PluginHostError::InternalError(format!("RPC error: {}", e)))??;
            }

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

        if let Some(process_arc) = &self.process {
            let result = {
                let process_guard = process_arc.lock().await;

                let ctx = context::current();
                process_guard
                    .client
                    .recreate_engine_instance(ctx, id, config)
                    .await
                    .map_err(|e| PluginHostError::InternalError(format!("RPC error: {}", e)))??
            };

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

        if let Some(process_arc) = &self.process {
            let process_guard = process_arc.lock().await;

            let mut ctx = context::current();
            ctx.deadline = std::time::Instant::now() + Duration::from_secs(20);

            process_guard
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

        if let Some(process_arc) = &self.process {
            let process_guard = process_arc.lock().await;

            let mut ctx = context::current();
            ctx.deadline = std::time::Instant::now() + Duration::from_secs(20);

            process_guard
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

        if let Some(process_arc) = &mut self.process {
            let mut process_guard = process_arc.lock().await;

            let ctx = context::current();
            let plugin_info = process_guard
                .client
                .get_plugin_info(ctx, self.plugin_id)
                .await
                .map_err(|e| PluginHostError::InternalError(format!("RPC error: {}", e)))??;

            process_guard.plugin_info = Some(plugin_info);

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

        // Return the stored info if available
        if let Some(process_arc) = &mut self.process {
            let process_guard = process_arc.lock().await;
            Ok(process_guard.plugin_info.clone())
        } else {
            Err(PluginHostError::InternalError(
                "Plugin host process not available".to_string(),
            ))
        }
    }

    // Helper method to start a new plugin host process
    async fn start_plugin_host_process(
        &self,
    ) -> Result<(Arc<Mutex<PluginHostProcess>>, String), Box<dyn std::error::Error>> {
        // Create bootstrap server to exchange transport information
        let (bootstrap_host, bootstrap_name) = BootstrapHost::new()?;
        info!("Created bootstrap server: {}", bootstrap_name);

        // Start the plugin host process
        let process = Command::new(&self.executable_path)
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
            plugin_info: None,
        };

        // Wrap in Arc<Mutex<>>
        let process_arc = Arc::new(Mutex::new(plugin_host_process));

        Ok((process_arc, bootstrap_name))
    }

    // Create and start the respawn task
    fn create_respawn_task(&mut self, process_arc: Arc<Mutex<PluginHostProcess>>) {
        let respawn_mode = self.respawn_mode.clone();
        let plugin_id = self.plugin_id;
        let executable_path = self.executable_path.clone();
        let plugin_path = self.plugin_path.clone();

        let task = tokio::spawn(async move {
            let mut consecutive_crashes: u64 = 0;

            loop {
                tokio::time::sleep(Duration::from_secs(3)).await;

                // Wait for the process to exit
                let _exit_status = {
                    match process_arc.lock().await.process.try_wait() {
                        Ok(Some(status)) => {
                            tracing::info!("Plugin host process exited with status: {:?}", status);
                            status
                        }
                        Ok(None) => {
                            // Process is still running, continue waiting
                            tracing::trace!("Plugin host process is still running, waiting...");
                            continue;
                        }
                        Err(e) => {
                            // If there was an error waiting, break the loop
                            tracing::error!("Error waiting for plugin host process: {}", e);
                            break;
                        }
                    }
                };

                // Check if we should respawn the process
                let mode = *respawn_mode.lock().await;
                match mode {
                    PluginHostRespawnMode::Suspended => {
                        tracing::info!("Plugin host process exited, but respawning is suspended");
                        // Sleep for a while before checking again if respawn mode has changed
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        continue;
                    }
                    PluginHostRespawnMode::NoRestart => {
                        tracing::info!("Plugin host process exited, not restarting");
                        break;
                    }
                    PluginHostRespawnMode::RestartOnCrash => {}
                }

                tracing::info!("Plugin host process crashed, restarting...");

                // Create bootstrap server to exchange transport information
                match BootstrapHost::new() {
                    Ok((bootstrap_host, bootstrap_name)) => {
                        // Use the extracted async function to handle respawn
                        let this = PluginHostConnection {
                            plugin_id,
                            plugin_path: plugin_path.clone(),
                            plugin_loaded: false,
                            respawn_mode: respawn_mode.clone(),
                            process: None,
                            respawn_task: None,
                            executable_path: executable_path.clone(),
                        };

                        match this
                            .handle_plugin_host_respawn(
                                process_arc.clone(),
                                bootstrap_host,
                                bootstrap_name,
                                executable_path.clone(),
                                plugin_id,
                                plugin_path.clone(),
                            )
                            .await
                        {
                            Ok(_) => {
                                // Reset consecutive crashes counter
                                consecutive_crashes = 0;

                                // Continue monitoring the new process
                            }
                            Err(e) => {
                                tracing::error!("Error during plugin host respawn: {}", e);

                                consecutive_crashes += 1;

                                if consecutive_crashes >= 5 {
                                    tracing::error!(
                                        "Plugin host process crashed too many times, suspending respawn: {}",
                                        e
                                    );
                                    let mut respawn_mode_guard = respawn_mode.lock().await;
                                    *respawn_mode_guard = PluginHostRespawnMode::Suspended;
                                    continue;
                                }

                                // Sleep to avoid respawning too quickly
                                tokio::time::sleep(Duration::from_secs(1)).await;
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("Failed to create bootstrap server for respawn: {}", e);

                        // Sleep to avoid respawning too quickly
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
            }

            tracing::info!("Respawn task for plugin {} exited", plugin_id);
        });

        self.respawn_task = Some(task);
    }

    // Handle respawning of a crashed plugin host process
    async fn handle_plugin_host_respawn(
        &self,
        process_arc: Arc<Mutex<PluginHostProcess>>,
        bootstrap_host: BootstrapHost<
            tarpc::ClientMessage<crate::plugin_host_service::PluginHostServiceRequest>,
            tarpc::Response<crate::plugin_host_service::PluginHostServiceResponse>,
        >,
        bootstrap_name: String,
        executable_path: PathBuf,
        plugin_id: Uuid,
        plugin_path: PathBuf,
    ) -> Result<(), std::io::Error> {
        tracing::info!("Created bootstrap server for respawn: {}", bootstrap_name);

        // Attempt to restart the process
        let new_process = Command::new(&executable_path)
            .arg("--mode=plugin_host")
            .arg(format!("--ipc={}", bootstrap_name))
            .spawn()?;

        let new_pid = new_process.id().unwrap_or(0);
        tracing::info!("Restarted plugin host process with PID: {}", new_pid);

        // Wait for the plugin host process to connect
        tracing::info!("Waiting for respawned plugin host process to connect...");

        let (transport, _connection) = bootstrap_host
            .accept(Duration::from_secs(10))
            .await
            .map_err(|err| {
                tracing::error!(
                    "Failed to accept connection from respawned plugin host: {}",
                    err
                );
                std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("Failed to accept connection: {}", err),
                )
            })?;

        tracing::info!("Respawned plugin host process connected");

        // Create the new RPC client
        let new_client = PluginHostServiceClient::new(tarpc::client::Config::default(), transport);
        let client = new_client.client;
        let dispatcher_task = tokio::spawn(async move {
            let _ = new_client.dispatch.await.inspect_err(|err| {
                tracing::error!("Error in dispatcher: {}", err);
            });
            tracing::info!("Dispatcher task finished");
        });

        // Update the PluginHostProcess with the new process and client
        let mut process_guard = process_arc.lock().await;

        process_guard.process = new_process;
        process_guard.client = client;
        process_guard._dispatcher_task = dispatcher_task;

        tracing::info!("Plugin host process and RPC client successfully recreated");

        // Try to reload the plugin
        let mut ctx = context::current();
        ctx.deadline = std::time::Instant::now() + Duration::from_secs(30);
        process_guard
            .client
            .load_plugin(ctx, plugin_id, plugin_path)
            .await
            .map_err(|e| {
                tracing::error!("RPC error while loading plugin after respawn: {}", e);
                std::io::Error::new(std::io::ErrorKind::Other, e)
            })?
            .map_err(|e| {
                tracing::error!("Failed to load plugin after respawn: {:?}", e);
                std::io::Error::new(
                    std::io::ErrorKind::Other,
                    std::format!("Failed to load plugin: {:?}", e),
                )
            })?;

        tracing::info!("Plugin reloaded successfully after respawn");

        // Restore engine instances from previously saved state
        if let Some(saved_plugin_info) = &process_guard.plugin_info {
            tracing::info!(
                "Attempting to restore {} engine instances after respawn",
                saved_plugin_info.engine_instances.len()
            );

            let plugin_info = process_guard
                .client
                .restore(ctx, process_guard.plugin_info.clone())
                .await
                .map_err(|e| {
                    tracing::error!("RPC error while restoring plugin info: {}", e);
                    std::io::Error::new(std::io::ErrorKind::Other, e)
                })?
                .map_err(|e| {
                    tracing::error!("Failed to restore plugin info after respawn: {:?}", e);
                    std::io::Error::new(
                        std::io::ErrorKind::Other,
                        std::format!("Failed to restore plugin info: {:?}", e),
                    )
                })?;

            process_guard.plugin_info = Some(plugin_info);

            tracing::info!("Successfully restored plugin state after respawn");
        } else {
            tracing::info!("No saved state to restore after respawn");
        }

        Ok(())
    }

    // Restore engine instances from saved plugin info
    pub async fn restore(&mut self) -> Result<PluginInfo, PluginHostError> {
        if !self.plugin_loaded {
            return Err(PluginHostError::PluginNotFound(
                "Plugin not loaded".to_string(),
            ));
        }

        if let Some(process_arc) = &self.process {
            let process_guard = process_arc.lock().await;

            // Create a context with timeout for the restore operation
            let mut ctx = context::current();
            ctx.deadline = std::time::Instant::now() + Duration::from_secs(30);

            // Call the restore method on the plugin host with the saved plugin info
            let result = process_guard
                .client
                .restore(ctx, process_guard.plugin_info.clone())
                .await
                .map_err(|e| {
                    PluginHostError::InternalError(format!("RPC error during restore: {}", e))
                })??;

            // Update the local plugin_info with the restored state
            let mut process_guard = process_arc.lock().await;
            process_guard.plugin_info = Some(result.clone());

            Ok(result)
        } else {
            Err(PluginHostError::InternalError(
                "Plugin host process not available".to_string(),
            ))
        }
    }
}

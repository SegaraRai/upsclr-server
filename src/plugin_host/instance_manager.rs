// Manages engine instances created from the loaded plugin
// Each instance is identified by a unique UUID

use crate::models::{
    ColorFormat, EngineInstance, EngineInstanceConfig, PluginHostError, PreloadParams,
    UpscaleParams, UpscaleResult,
};
use crate::plugin_host::plugin::LoadedPlugin;
use crate::plugin_host::plugin_ffi;
use ipc_channel::ipc::IpcSharedMemory;
use std::collections::HashMap;
use std::ffi::CString;
use std::sync::{Arc, RwLock};
use std::time::Instant;
use tracing::{debug, info};
use uuid::Uuid;

// Represents an active, instantiated engine from a plugin
pub struct ActiveInstance {
    pub instance_id: Uuid,
    // The opaque pointer to the C engine instance, managed by the plugin
    pub instance_ptr: *mut plugin_ffi::UpsclrEngineInstance,
    // The configuration used to create this instance
    pub config: EngineInstanceConfig,
    // Creation time
    pub created_at: Instant,
}

// Explicitly implement thread-safety traits
// Safety: The underlying plugin library handles thread safety for the instance_ptr
unsafe impl Send for ActiveInstance {}
unsafe impl Sync for ActiveInstance {}

// Implement Drop trait to automatically clean up resources when an instance is dropped
impl Drop for ActiveInstance {
    fn drop(&mut self) {
        // Ensure the pointer is not null before calling destroy
        if !self.instance_ptr.is_null() {
            debug!("Dropping engine instance: {}", self.instance_id);
            // We need a reference to the plugin to call destroy
            // But we don't have it here - the InstanceManager handles this
        }
    }
}

// Manages the collection of all active engine instances
pub struct InstanceManager {
    // Reference to the loaded plugin
    plugin: Arc<LoadedPlugin>,
    // Maps UUIDs to their corresponding active engine instances
    instances: RwLock<HashMap<Uuid, Arc<ActiveInstance>>>,
}

impl InstanceManager {
    // Create a new instance manager for the given plugin
    pub fn new(plugin: Arc<LoadedPlugin>) -> Self {
        Self {
            plugin,
            instances: RwLock::new(HashMap::new()),
        }
    }

    // Create a new engine instance
    pub fn create_instance(
        &self,
        config: EngineInstanceConfig,
    ) -> Result<EngineInstance, PluginHostError> {
        // Find the engine by name
        let engine_name = &config.engine_name;
        let engine_index = self
            .plugin
            .find_engine_index(engine_name)
            .ok_or_else(|| PluginHostError::EngineNotFound(engine_name.to_string()))?;

        // Convert the config to a JSON string
        let config_json = config.engine_config.to_string();

        // Convert to CString for FFI
        let config_c_str = CString::new(config_json.as_bytes()).map_err(|e| {
            PluginHostError::InvalidParameter(format!("Invalid config JSON: {}", e))
        })?;

        // Call the plugin to create the instance
        let instance_ptr = unsafe {
            (self.plugin.upsclr_plugin_create_engine_instance)(
                engine_index,
                config_c_str.as_ptr(),
                config_json.len(),
            )
        };

        // Check if instance creation succeeded
        if instance_ptr.is_null() {
            return Err(PluginHostError::EngineCreationFailed(
                "Plugin returned NULL pointer for engine instance".to_string(),
            ));
        }

        // Create the active instance
        let active_instance = Arc::new(ActiveInstance {
            instance_id: config.instance_id,
            instance_ptr,
            config: config.clone(),
            created_at: Instant::now(),
        });

        // Store the instance
        self.instances
            .write()
            .unwrap()
            .insert(config.instance_id, active_instance);

        // Return the engine instance info
        Ok(EngineInstance {
            config,
            uptime_ms: 0, // Just created
        })
    }

    // Get an instance by ID
    pub fn get_instance(&self, id: &Uuid) -> Option<Arc<ActiveInstance>> {
        self.instances.read().unwrap().get(id).cloned()
    }

    // Destroy an instance
    pub fn destroy_instance(&self, id: &Uuid) -> Result<(), PluginHostError> {
        let mut instances = self.instances.write().unwrap();

        if let Some(instance) = instances.remove(id) {
            // Call the plugin to destroy the instance
            unsafe {
                (self.plugin.upsclr_plugin_destroy_engine_instance)(instance.instance_ptr);
            }

            info!("Destroyed engine instance: {}", id);
            Ok(())
        } else {
            Err(PluginHostError::EngineInstanceNotFound(*id))
        }
    }

    // Get all instance IDs
    pub fn get_instance_ids(&self) -> Vec<Uuid> {
        self.instances.read().unwrap().keys().cloned().collect()
    }

    // Get a list of all engine instances
    pub fn get_all_instances(&self) -> Vec<EngineInstance> {
        self.instances
            .read()
            .unwrap()
            .values()
            .map(|instance| EngineInstance {
                config: instance.config.clone(),
                uptime_ms: instance.created_at.elapsed().as_millis() as u64,
            })
            .collect()
    }

    // Recreate an instance with new config
    pub fn recreate_instance(
        &self,
        id: &Uuid,
        config: Option<EngineInstanceConfig>,
    ) -> Result<EngineInstance, PluginHostError> {
        // First check if the instance exists
        let instance = self
            .get_instance(id)
            .ok_or(PluginHostError::EngineInstanceNotFound(*id))?;

        // Use new config if provided, otherwise use the existing config
        let new_config = config.unwrap_or_else(|| instance.config.clone());

        // Destroy the existing instance
        self.destroy_instance(id)?;

        // Create a new instance with the new or existing config
        self.create_instance(new_config)
    }

    // Run preload operation on an instance
    pub fn preload(&self, id: &Uuid, params: PreloadParams) -> Result<(), PluginHostError> {
        // Get the instance
        let instance = self
            .get_instance(id)
            .ok_or(PluginHostError::EngineInstanceNotFound(*id))?;

        // Call the plugin's preload function
        let result = unsafe {
            (self.plugin.upsclr_plugin_preload_upscale)(
                instance.instance_ptr,
                params.width.max(params.height) as i32, // Use max dimension as scale factor
            )
        };

        if result != plugin_ffi::UpsclrErrorCode::Success {
            return Err(PluginHostError::OperationFailed(format!(
                "Preload operation failed with error code: {:?}",
                result
            )));
        }

        Ok(())
    }

    // Run upscale operation on an instance
    pub fn upscale(
        &self,
        id: &Uuid,
        params: UpscaleParams,
    ) -> Result<UpscaleResult, PluginHostError> {
        // Get the instance
        let instance = self
            .get_instance(id)
            .ok_or(PluginHostError::EngineInstanceNotFound(*id))?;

        // Convert ColorFormat to plugin_ffi::UpsclrColorFormat
        let input_color = match params.input_color_format {
            ColorFormat::Rgb => plugin_ffi::UpsclrColorFormat::Rgb,
            ColorFormat::Bgr => plugin_ffi::UpsclrColorFormat::Bgr,
        };

        let output_color = match params.desired_color_format {
            ColorFormat::Rgb => plugin_ffi::UpsclrColorFormat::Rgb,
            ColorFormat::Bgr => plugin_ffi::UpsclrColorFormat::Bgr,
        };

        // Calculate expected output dimensions based on scale
        let expected_width = params.input_width * params.scale;
        let expected_height = params.input_height * params.scale;
        let expected_size = (expected_width * expected_height * params.channels) as usize;

        // Allocate buffer for output data
        let mut output_data = vec![0u8; expected_size];

        // Record start time for performance measurement
        let start_time = Instant::now();

        // Call the plugin's upscale function
        let result = unsafe {
            (self.plugin.upsclr_plugin_upscale)(
                instance.instance_ptr,
                params.scale as i32,
                params.input_data.as_ptr(),
                params.input_data.len(),
                params.input_width,
                params.input_height,
                params.channels,
                input_color,
                output_data.as_mut_ptr(),
                output_data.len(),
                output_color,
            )
        };

        // Record end time and calculate processing time
        let processing_time_us = start_time.elapsed().as_micros() as u64;

        if result != plugin_ffi::UpsclrErrorCode::Success {
            return Err(PluginHostError::OperationFailed(format!(
                "Upscale operation failed with error code: {:?}",
                result
            )));
        }

        // Create and return the result
        Ok(UpscaleResult {
            processing_time_us,
            output_width: expected_width,
            output_height: expected_height,
            channels: params.channels,
            output_color_format: params.desired_color_format,
            output_data: IpcSharedMemory::from_bytes(&output_data),
        })
    }

    // Cleanup all instances
    pub fn cleanup(&self) {
        let mut instances = self.instances.write().unwrap();

        for (id, instance) in instances.drain() {
            unsafe {
                (self.plugin.upsclr_plugin_destroy_engine_instance)(instance.instance_ptr);
            }
            debug!("Destroyed engine instance during cleanup: {}", id);
        }

        info!("Cleaned up all engine instances");
    }
}

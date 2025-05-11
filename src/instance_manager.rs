// Manages active engine instances created from loaded plugins.
// Each instance is identified by a unique UUID.

use crate::error::AppError; // For specific error types
use crate::models::InstanceInfoForList; // For listing instances
use crate::plugin_ffi;
use crate::plugin_manager::LoadedPlugin;
use std::collections::HashMap;
use std::ffi::CString;
use std::sync::Arc; // For shared ownership of ActiveInstance
use uuid::Uuid;

// Represents an active, instantiated engine from a plugin.
// Since plugins are thread-safe, this struct itself doesn't need a Mutex
// for plugin call safety.
pub struct ActiveInstance {
    pub uuid: Uuid,
    // The opaque pointer to the C engine instance, managed by the plugin.
    pub instance_ptr: *mut plugin_ffi::UpsclrEngineInstance,
    // Arc reference to the LoadedPlugin that created this instance.
    // This keeps the plugin library loaded and provides access to its functions.
    pub plugin: Arc<LoadedPlugin>,
    // The index of the engine type within the plugin's list of engines.
    pub engine_index_in_plugin: usize,
    // Store the JSON configuration string used to create this instance, for reference.
    pub config_json_string: String,
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
            tracing::info!("Dropping engine instance: {}", self.uuid);
            // Call the plugin's FFI function to destroy the instance
            unsafe {
                (self.plugin.upsclr_plugin_destroy_engine_instance)(self.instance_ptr);
            }
        }
    }
}

// Manages the collection of all active engine instances.
// The InstanceManager itself (specifically, its `instances` HashMap)
// should be protected by a Mutex if accessed concurrently by multiple Axum tasks.
#[derive(Default)]
pub struct InstanceManager {
    // Maps UUIDs to their corresponding active engine instances.
    // Arc<ActiveInstance> allows multiple parts of the system to reference an instance
    // if needed, though typically operations are per-request.
    instances: HashMap<Uuid, Arc<ActiveInstance>>,
}

impl InstanceManager {
    // Creates a new engine instance using the specified plugin and configuration.
    // This involves calling the plugin's FFI function to create the instance.
    pub fn create_instance(
        &mut self,
        plugin: Arc<LoadedPlugin>,
        engine_index: usize,
        config_json_str: &str, // The validated JSON configuration string.
    ) -> Result<Arc<ActiveInstance>, AppError> {
        // Convert the Rust &str to a null-terminated CString for FFI.
        let config_json_c_str = CString::new(config_json_str).map_err(AppError::CStringError)?; // Handles null bytes in string

        // Call the plugin's FFI function to create the instance. This is unsafe.
        let instance_ptr = unsafe {
            (plugin.upsclr_plugin_create_engine_instance)(
                engine_index,
                config_json_c_str.as_ptr(), // Pass as *const c_char
                config_json_c_str.as_bytes().len(), // Length of string data (excluding null terminator)
            )
        };

        // Check if the plugin successfully created the instance.
        if instance_ptr.is_null() {
            return Err(AppError::InstanceCreationFailed(
                "Plugin returned a null pointer, indicating instance creation failed.".to_string(),
            ));
        }

        let uuid = Uuid::new_v4();
        let active_instance = Arc::new(ActiveInstance {
            uuid,
            instance_ptr,
            plugin: plugin.clone(), // Clone the Arc to share ownership.
            engine_index_in_plugin: engine_index,
            config_json_string: config_json_str.to_string(),
        });

        // Store the new active instance.
        self.instances.insert(uuid, active_instance.clone());
        tracing::info!("Created and registered new engine instance: {}", uuid);
        Ok(active_instance)
    }

    // Retrieves a shared reference to an active instance by its UUID.
    pub fn get_instance(&self, uuid: &Uuid) -> Option<Arc<ActiveInstance>> {
        self.instances.get(uuid).cloned()
    }

    // Returns a list of simplified information for all currently active instances.
    // Used by the GET /instances API endpoint.
    pub fn list_instances_info(&self) -> Vec<InstanceInfoForList> {
        self.instances
            .values()
            .map(|instance_arc| {
                let plugin_name = instance_arc.plugin.plugin_info.name.clone();
                // Ensure engine_index_in_plugin is valid before indexing.
                let engine_name = instance_arc
                    .plugin
                    .engines_info
                    .get(instance_arc.engine_index_in_plugin)
                    .map_or_else(
                        || "Unknown Engine".to_string(),
                        |e_info| e_info.name.clone(),
                    );

                InstanceInfoForList {
                    instance_id: instance_arc.uuid,
                    plugin_name,
                    engine_name,
                }
            })
            .collect()
    }

    // Destroys an engine instance and removes it from active management.
    // The actual resource cleanup is handled by the Drop implementation on ActiveInstance.
    pub fn delete_instance(&mut self, uuid: &Uuid) -> Result<(), AppError> {
        if let Some(_instance_arc) = self.instances.remove(uuid) {
            // The instance will be automatically destroyed when _instance_arc is dropped
            // at the end of this scope, thanks to the Drop trait implementation.
            tracing::info!("Unregistered engine instance: {}", uuid);
            Ok(())
        } else {
            // If instance not found, it's a client error (or race condition).
            Err(AppError::InstanceNotFound) // Use the specific error
        }
    }
}

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
            tracing::debug!("Dropping engine instance: {}", self.uuid);
            // Call the plugin's FFI function to destroy the instance
            unsafe {
                (self.plugin.upsclr_plugin_destroy_engine_instance)(self.instance_ptr);
            }
        }
        tracing::debug!("Dropped engine instance: {}", self.uuid);
    }
}

/// An engine instance state for import/export.
#[derive(Debug, Clone)]
pub struct InstanceState {
    pub instance_id: Uuid,
    pub plugin_id: String,
    pub engine_index_in_plugin: usize,
    pub config_json_string: String,
}

/// Container for exporting all instances at once.
#[derive(Debug, Clone)]
pub struct InstanceManagerState {
    pub instances: Vec<InstanceState>,
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
                let plugin_id = instance_arc.plugin.plugin_info.id.clone();
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
                    plugin_id,
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

    pub fn cleanup(&mut self, wait: bool) {
        if wait {
            tracing::debug!("Waiting for all engine instances to finish...");

            // Create a weak reference to each instance to ensure they are dropped
            let weak_refs: Vec<_> = self.instances.values().map(Arc::downgrade).collect();

            // Clear the instances map, which will drop all instances
            // This will not immediately drop them, but will allow them to be dropped when there are no more strong references.
            self.instances.clear();

            // Wait for all instances to be dropped
            for weak_ref in weak_refs {
                // This will block until the instance is dropped
                while weak_ref.upgrade().is_some() {
                    std::thread::yield_now(); // Yield to allow other threads to run
                }
            }

            tracing::info!("All engine instances have been cleared.");
        } else {
            // This will drop all instances, invoking their Drop implementation.
            self.instances.clear();

            tracing::info!("Cleared all engine instances.");
        }
    }

    // Export the current state of all instances.
    pub fn export_state(&self) -> InstanceManagerState {
        let instances = self
            .instances
            .values()
            .map(|instance_arc| InstanceState {
                instance_id: instance_arc.uuid,
                plugin_id: instance_arc.plugin.plugin_info.id.clone(),
                engine_index_in_plugin: instance_arc.engine_index_in_plugin,
                config_json_string: instance_arc.config_json_string.clone(),
            })
            .collect();

        InstanceManagerState { instances }
    }

    // Import instances from a previously exported state.
    pub fn import_state(
        &mut self,
        state: InstanceManagerState,
        plugin_manager: &crate::plugin_manager::PluginManager,
    ) -> Result<Vec<Uuid>, AppError> {
        // Clear existing instances before importing
        if !self.instances.is_empty() {
            tracing::warn!("Clearing existing instances before import.");
            self.cleanup(true);
        }

        if state.instances.is_empty() {
            tracing::info!("Importing empty instance state, nothing to do.");
        }

        let mut imported_ids = Vec::new();

        for instance_state in state.instances {
            // Skip instances that already exist (using the same UUID)
            if self.instances.contains_key(&instance_state.instance_id) {
                tracing::warn!(
                    "Instance with ID {} already exists, skipping import.",
                    instance_state.instance_id
                );
                continue;
            }

            // Find the plugin
            let plugin = match plugin_manager.find_plugin_by_id(&instance_state.plugin_id) {
                Some(p) => p,
                None => {
                    tracing::warn!(
                        "Plugin with ID {} not found, skipping instance {}.",
                        instance_state.plugin_id,
                        instance_state.instance_id
                    );
                    continue;
                }
            };

            // Validate engine index
            if instance_state.engine_index_in_plugin >= plugin.engines_info.len() {
                tracing::warn!(
                    "Invalid engine index {} for plugin {}, skipping instance {}.",
                    instance_state.engine_index_in_plugin,
                    instance_state.plugin_id,
                    instance_state.instance_id
                );
                continue;
            }

            // Create a CString for FFI
            let config_json_c_str = match CString::new(&instance_state.config_json_string[..]) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(
                        "Failed to create CString for instance {}: {}",
                        instance_state.instance_id,
                        e
                    );
                    continue;
                }
            };

            // Call the plugin to create the instance
            let instance_ptr = unsafe {
                (plugin.upsclr_plugin_create_engine_instance)(
                    instance_state.engine_index_in_plugin,
                    config_json_c_str.as_ptr(),
                    config_json_c_str.as_bytes().len(),
                )
            };

            // Check if instance creation succeeded
            if instance_ptr.is_null() {
                tracing::warn!(
                    "Failed to create instance {}, plugin returned null pointer.",
                    instance_state.instance_id
                );
                continue;
            }

            // Create and store the ActiveInstance with the original UUID
            let active_instance = Arc::new(ActiveInstance {
                uuid: instance_state.instance_id,
                instance_ptr,
                plugin: plugin.clone(),
                engine_index_in_plugin: instance_state.engine_index_in_plugin,
                config_json_string: instance_state.config_json_string,
            });

            self.instances
                .insert(instance_state.instance_id, active_instance);
            imported_ids.push(instance_state.instance_id);

            tracing::info!("Imported engine instance: {}", instance_state.instance_id);
        }

        Ok(imported_ids)
    }
}

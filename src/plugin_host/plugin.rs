// Manages the loading of a plugin from a shared library,
// retrieving its metadata and function pointers for FFI calls.

use crate::models::{EngineInfo, PluginHostError, PluginInfo};
use crate::plugin_host::plugin_ffi;
use libloading::{Library, Symbol};
use serde_json::Value;
use std::ffi::{CStr, CString};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, info, trace};
use uuid::Uuid;

pub struct PluginLibrary {
    _lib: Library,
    pub id: Uuid,
    pub path: PathBuf,
    #[allow(unused)]
    upsclr_plugin_initialize: unsafe extern "C" fn() -> plugin_ffi::UpsclrErrorCode,
    upsclr_plugin_shutdown: unsafe extern "C" fn() -> plugin_ffi::UpsclrErrorCode,
}

impl Drop for PluginLibrary {
    fn drop(&mut self) {
        // Call the shutdown function when the library is dropped.
        info!("Dropping plugin library: {}", self.id);
        unsafe {
            (self.upsclr_plugin_shutdown)();
        }
        debug!("Dropped plugin library: {}", self.id);
    }
}

// Represents a successfully loaded plugin and its capabilities.
// This struct holds the loaded library and symbols to its functions.
#[derive(Clone)]
pub struct LoadedPlugin {
    _lib: Arc<PluginLibrary>, // Arc to keep the library loaded as long as this struct (or clones) exist.

    pub id: Uuid,            // Unique identifier assigned by the main process.
    pub path: PathBuf,       // Filesystem path to the loaded library.
    pub start_time: Instant, // When the plugin was loaded

    // Rust-native, readily usable information extracted from the plugin.
    pub plugin_info: PluginInfo,

    // Instead of storing the Symbol itself, we store function pointers that are type-erased and safe to share between threads
    pub upsclr_plugin_get_info: unsafe extern "C" fn() -> *const plugin_ffi::UpsclrPluginInfo,
    pub upsclr_plugin_count_engines: unsafe extern "C" fn() -> usize,
    pub upsclr_plugin_get_engine_info:
        unsafe extern "C" fn(engine_index: usize) -> *const plugin_ffi::UpsclrEngineInfo,
    pub upsclr_plugin_validate_engine_config:
        unsafe extern "C" fn(
            engine_index: usize,
            config_json: *const plugin_ffi::c_char8_t,
            config_json_length: usize,
        ) -> *const plugin_ffi::UpsclrEngineConfigValidationResult,
    pub upsclr_plugin_free_validation_result:
        unsafe extern "C" fn(result: *const plugin_ffi::UpsclrEngineConfigValidationResult),
    pub upsclr_plugin_create_engine_instance:
        unsafe extern "C" fn(
            engine_index: usize,
            config_json: *const plugin_ffi::c_char8_t,
            config_json_length: usize,
        ) -> *mut plugin_ffi::UpsclrEngineInstance,
    pub upsclr_plugin_destroy_engine_instance:
        unsafe extern "C" fn(instance: *mut plugin_ffi::UpsclrEngineInstance),
    pub upsclr_plugin_preload_upscale: unsafe extern "C" fn(
        instance: *mut plugin_ffi::UpsclrEngineInstance,
        scale: i32,
    ) -> plugin_ffi::UpsclrErrorCode,
    pub upsclr_plugin_upscale: unsafe extern "C" fn(
        instance: *mut plugin_ffi::UpsclrEngineInstance,
        scale: i32,
        in_data: *const u8,
        in_size: usize,
        in_width: u32,
        in_height: u32,
        in_channels: u32,
        in_color_format: plugin_ffi::UpsclrColorFormat,
        out_data: *mut u8,
        out_size: usize,
        out_color_format: plugin_ffi::UpsclrColorFormat,
    ) -> plugin_ffi::UpsclrErrorCode,
}

// Implement thread-safety traits - required since we'll be using this across threads
unsafe impl Send for LoadedPlugin {}
unsafe impl Sync for LoadedPlugin {}

impl LoadedPlugin {
    // Helper function to safely convert a C-style string (UTF-8 encoded) to a Rust String.
    // Returns a Result to handle potential null pointers or invalid UTF-8.
    unsafe fn c_str_to_rust_string(
        c_str_ptr: *const plugin_ffi::c_char8_t,
    ) -> Result<String, String> {
        if c_str_ptr.is_null() {
            // Decide how to handle nulls: empty string or an error.
            // Returning an error is often safer to highlight plugin issues.
            return Err("Encountered a null string pointer from plugin".to_string());
        }
        CStr::from_ptr(c_str_ptr)
            .to_str()
            .map(String::from)
            .map_err(|e| format!("Invalid UTF-8 sequence in string from plugin: {}", e))
    }

    // Load a plugin from the specified path
    pub fn load(plugin_id: Uuid, path: PathBuf) -> Result<Self, PluginHostError> {
        unsafe {
            // Load the shared library.
            let lib = Library::new(&path).map_err(|e| {
                PluginHostError::PluginLoadError(format!(
                    "Failed to load shared library from {:?}: {}",
                    path, e
                ))
            })?;

            // Macro to simplify symbol loading and error mapping.
            macro_rules! get_symbol {
                ($lib:expr, $name:expr) => {
                    $lib.get($name).map_err(|e| {
                        PluginHostError::PluginLoadError(format!(
                            "Failed to load symbol '{}' from {:?}: {}",
                            String::from_utf8_lossy($name),
                            path,
                            e
                        ))
                    })
                };
            }

            // Load all required function pointers from the library.
            let initialize_fn: Symbol<unsafe extern "C" fn() -> plugin_ffi::UpsclrErrorCode> =
                get_symbol!(lib, b"upsclr_plugin_initialize\0")?;
            let shutdown_fn: Symbol<unsafe extern "C" fn() -> plugin_ffi::UpsclrErrorCode> =
                get_symbol!(lib, b"upsclr_plugin_shutdown\0")?;
            let get_info_fn: Symbol<unsafe extern "C" fn() -> *const plugin_ffi::UpsclrPluginInfo> =
                get_symbol!(lib, b"upsclr_plugin_get_info\0")?;
            let count_engines_fn: Symbol<unsafe extern "C" fn() -> usize> =
                get_symbol!(lib, b"upsclr_plugin_count_engines\0")?;
            let get_engine_info_fn: Symbol<
                unsafe extern "C" fn(usize) -> *const plugin_ffi::UpsclrEngineInfo,
            > = get_symbol!(lib, b"upsclr_plugin_get_engine_info\0")?;
            let validate_config_fn: Symbol<
                unsafe extern "C" fn(
                    usize,
                    *const plugin_ffi::c_char8_t,
                    usize,
                )
                    -> *const plugin_ffi::UpsclrEngineConfigValidationResult,
            > = get_symbol!(lib, b"upsclr_plugin_validate_engine_config\0")?;
            let free_validation_result_fn: Symbol<
                unsafe extern "C" fn(*const plugin_ffi::UpsclrEngineConfigValidationResult),
            > = get_symbol!(lib, b"upsclr_plugin_free_validation_result\0")?;
            let create_instance_fn: Symbol<
                unsafe extern "C" fn(
                    usize,
                    *const plugin_ffi::c_char8_t,
                    usize,
                ) -> *mut plugin_ffi::UpsclrEngineInstance,
            > = get_symbol!(lib, b"upsclr_plugin_create_engine_instance\0")?;
            let destroy_instance_fn: Symbol<
                unsafe extern "C" fn(*mut plugin_ffi::UpsclrEngineInstance),
            > = get_symbol!(lib, b"upsclr_plugin_destroy_engine_instance\0")?;
            let preload_fn: Symbol<
                unsafe extern "C" fn(
                    *mut plugin_ffi::UpsclrEngineInstance,
                    i32,
                ) -> plugin_ffi::UpsclrErrorCode,
            > = get_symbol!(lib, b"upsclr_plugin_preload_upscale\0")?;
            let upscale_fn: Symbol<
                unsafe extern "C" fn(
                    *mut plugin_ffi::UpsclrEngineInstance,
                    i32,
                    *const u8,
                    usize,
                    u32,
                    u32,
                    u32,
                    plugin_ffi::UpsclrColorFormat,
                    *mut u8,
                    usize,
                    plugin_ffi::UpsclrColorFormat,
                ) -> plugin_ffi::UpsclrErrorCode,
            > = get_symbol!(lib, b"upsclr_plugin_upscale\0")?;

            // Call `upsclr_plugin_initialize` to initialize the plugin.
            trace!("{:?}: Calling upsclr_plugin_initialize", path);
            let init_result = initialize_fn();
            if init_result != plugin_ffi::UpsclrErrorCode::Success {
                return Err(PluginHostError::PluginLoadError(format!(
                    "Plugin at {:?} failed to initialize with error code: {:?}",
                    path, init_result
                )));
            }

            // Call `upsclr_plugin_get_info` to retrieve basic plugin metadata.
            trace!("{:?}: Calling upsclr_plugin_get_info", path);
            let c_plugin_info_ptr = get_info_fn();
            if c_plugin_info_ptr.is_null() {
                return Err(PluginHostError::PluginLoadError(format!(
                    "Plugin at {:?} returned NULL from upsclr_plugin_get_info",
                    path
                )));
            }
            let c_plugin_info = &*c_plugin_info_ptr; // Unsafe dereference. Valid if plugin API is correct.

            // Extract supported engines
            trace!("{:?}: Calling upsclr_plugin_count_engines", path);
            let mut engines_info = Vec::new();
            let num_engines = count_engines_fn();
            for i in 0..num_engines {
                trace!(
                    "{:?}: Calling upsclr_plugin_get_engine_info for engine index {}",
                    path, i
                );
                let c_engine_info_ptr = get_engine_info_fn(i);
                if c_engine_info_ptr.is_null() {
                    return Err(PluginHostError::PluginLoadError(format!(
                        "Plugin at {:?} returned NULL from upsclr_plugin_get_engine_info for engine index {}",
                        path, i
                    )));
                }
                let c_engine_info = &*c_engine_info_ptr; // Unsafe dereference.

                let name = Self::c_str_to_rust_string(c_engine_info.name).map_err(|e| {
                    PluginHostError::PluginLoadError(format!(
                        "Invalid engine name (idx {}) from {:?}: {}",
                        i, path, e
                    ))
                })?;

                // Parse JSON schema
                let schema_str = Self::c_str_to_rust_string(c_engine_info.config_json_schema)
                    .map_err(|e| {
                        PluginHostError::PluginLoadError(format!(
                            "Invalid engine schema (idx {}) from {:?}: {}",
                            i, path, e
                        ))
                    })?;

                let schema_json: Value = serde_json::from_str(&schema_str).map_err(|e| {
                    PluginHostError::PluginLoadError(format!(
                        "Invalid JSON schema (idx {}) from {:?}: {:?}",
                        i, path, e
                    ))
                })?;

                let schema_json_str = serde_json::to_string(&schema_json).map_err(|e| {
                    PluginHostError::PluginLoadError(format!(
                        "Failed to serialize engine schema (idx {}) from {:?}: {:?}",
                        i, path, e
                    ))
                })?;

                engines_info.push(EngineInfo {
                    engine_name: name,
                    engine_config_schema: schema_json_str,
                });
            }

            // Create PluginInfo
            let plugin_info = PluginInfo {
                plugin_id,
                path: path.clone(),
                name: Self::c_str_to_rust_string(c_plugin_info.name).map_err(|e| {
                    PluginHostError::PluginLoadError(format!(
                        "Invalid plugin name from {:?}: {}",
                        path, e
                    ))
                })?,
                version: Self::c_str_to_rust_string(c_plugin_info.version).map_err(|e| {
                    PluginHostError::PluginLoadError(format!(
                        "Invalid plugin version from {:?}: {}",
                        path, e
                    ))
                })?,
                author: "".to_string(), // This field is not in the old API, fill with empty string
                description: Self::c_str_to_rust_string(c_plugin_info.description).map_err(
                    |e| {
                        PluginHostError::PluginLoadError(format!(
                            "Invalid plugin description from {:?}: {}",
                            path, e
                        ))
                    },
                )?,
                supported_engines: engines_info,
                engine_instances: Vec::new(), // Initially empty
            };

            // Extract raw function pointers before building the struct
            let initialize_fn_ptr = *initialize_fn;
            let shutdown_fn_ptr = *shutdown_fn;
            let get_info_fn_ptr = *get_info_fn;
            let count_engines_fn_ptr = *count_engines_fn;
            let get_engine_info_fn_ptr = *get_engine_info_fn;
            let validate_config_fn_ptr = *validate_config_fn;
            let free_validation_result_fn_ptr = *free_validation_result_fn;
            let create_instance_fn_ptr = *create_instance_fn;
            let destroy_instance_fn_ptr = *destroy_instance_fn;
            let preload_fn_ptr = *preload_fn;
            let upscale_fn_ptr = *upscale_fn;

            let plugin_lib_arc = Arc::new(PluginLibrary {
                _lib: lib,
                id: plugin_id,
                path: path.clone(),
                upsclr_plugin_initialize: initialize_fn_ptr,
                upsclr_plugin_shutdown: shutdown_fn_ptr,
            });

            // Construct and return the LoadedPlugin struct
            Ok(LoadedPlugin {
                _lib: plugin_lib_arc,
                id: plugin_id,
                path,
                start_time: Instant::now(),
                plugin_info,

                // Use the extracted function pointers
                upsclr_plugin_get_info: get_info_fn_ptr,
                upsclr_plugin_count_engines: count_engines_fn_ptr,
                upsclr_plugin_get_engine_info: get_engine_info_fn_ptr,
                upsclr_plugin_validate_engine_config: validate_config_fn_ptr,
                upsclr_plugin_free_validation_result: free_validation_result_fn_ptr,
                upsclr_plugin_create_engine_instance: create_instance_fn_ptr,
                upsclr_plugin_destroy_engine_instance: destroy_instance_fn_ptr,
                upsclr_plugin_preload_upscale: preload_fn_ptr,
                upsclr_plugin_upscale: upscale_fn_ptr,
            })
        }
    }

    // Find an engine by its name
    pub fn find_engine_index(&self, engine_name: &str) -> Option<usize> {
        self.plugin_info
            .supported_engines
            .iter()
            .position(|engine| engine.engine_name == engine_name)
    }

    // Validate engine configuration
    pub fn validate_engine_config(
        &self,
        engine_name: &str,
        config: Value,
    ) -> Result<crate::models::EngineConfigValidationResult, PluginHostError> {
        // Find the engine index by name
        let engine_index = self
            .find_engine_index(engine_name)
            .ok_or_else(|| PluginHostError::EngineNotFound(engine_name.to_string()))?;

        // Convert config to a JSON string
        let config_json = config.to_string();
        let config_c_str = CString::new(config_json.as_bytes()).map_err(|e| {
            PluginHostError::InvalidParameter(format!("Invalid config JSON: {}", e))
        })?;

        // Call the plugin's validation function
        let validation_result_ptr = unsafe {
            (self.upsclr_plugin_validate_engine_config)(
                engine_index,
                config_c_str.as_ptr(),
                config_json.len(),
            )
        };

        if validation_result_ptr.is_null() {
            return Err(PluginHostError::OperationFailed(
                "Plugin returned NULL from validation function.".to_string(),
            ));
        }

        // Process the validation result
        let result = unsafe { &*validation_result_ptr };

        // Extract warning messages
        let mut warning_messages = Vec::new();
        if !result.warning_messages.is_null() && result.warning_count > 0 {
            for i in 0..result.warning_count {
                let msg_ptr = unsafe { *result.warning_messages.add(i) };
                if !msg_ptr.is_null() {
                    if let Ok(msg) = unsafe { Self::c_str_to_rust_string(msg_ptr) } {
                        warning_messages.push(msg);
                    }
                }
            }
        }

        // Extract error messages
        let mut error_messages = Vec::new();
        if !result.error_messages.is_null() && result.error_count > 0 {
            for i in 0..result.error_count {
                let msg_ptr = unsafe { *result.error_messages.add(i) };
                if !msg_ptr.is_null() {
                    if let Ok(msg) = unsafe { Self::c_str_to_rust_string(msg_ptr) } {
                        error_messages.push(msg);
                    }
                }
            }
        }

        // Create the validation result
        let validation_result = crate::models::EngineConfigValidationResult {
            is_valid: result.is_valid,
            warning_messages,
            error_messages,
        };

        // Free the validation result
        unsafe { (self.upsclr_plugin_free_validation_result)(validation_result_ptr) };

        Ok(validation_result)
    }
}

// Manages the loading of dynamic plugins from shared libraries,
// retrieving their metadata and function pointers for FFI calls.

use crate::error::AppError; // For error reporting
use crate::plugin_ffi;
use libloading::{Library, Symbol};
use serde::Serialize; // For PluginInfo, EngineInfo if they need to be serialized directly
use std::collections::HashMap;
use std::ffi::{CStr, OsStr};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub struct PluginLibrary {
    _lib: Library,
    id: String,
    #[allow(unused)]
    upsclr_plugin_initialize: unsafe extern "C" fn() -> plugin_ffi::UpsclrErrorCode,
    upsclr_plugin_shutdown: unsafe extern "C" fn() -> plugin_ffi::UpsclrErrorCode,
}

impl Drop for PluginLibrary {
    fn drop(&mut self) {
        // Call the shutdown function when the library is dropped.
        tracing::info!("Dropping plugin library: {}", self.id);
        unsafe {
            (self.upsclr_plugin_shutdown)();
        }
        tracing::debug!("Dropped plugin library: {}", self.id);
    }
}

// Represents a successfully loaded plugin and its capabilities.
// This struct holds the loaded library and symbols to its functions.
#[derive(Clone)]
pub struct LoadedPlugin {
    _lib: Arc<PluginLibrary>, // Arc to keep the library loaded as long as this struct (or clones) exist.

    pub id: String,    // Unique identifier, typically the plugin's filename.
    pub path: PathBuf, // Filesystem path to the loaded library.

    // Rust-native, readily usable information extracted from the plugin.
    pub plugin_info: PluginInfo,
    pub engines_info: Vec<EngineInfo>,

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

// These traits are automatically implemented for raw function pointers
// unsafe impl Send for LoadedPlugin {}
// unsafe impl Sync for LoadedPlugin {}

// Rust-native representation of basic plugin information.
#[derive(Serialize, Clone, Debug)]
pub struct PluginInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
}

// Rust-native representation of an upscale engine's information.
#[derive(Serialize, Clone, Debug)]
pub struct EngineInfo {
    pub name: String,
    pub description: String,
    pub version: String,
    pub config_schema: serde_json::Value, // JSON schema as a string.
    pub plugin_id: String,                // ID of the plugin this engine belongs to.
    pub engine_index_in_plugin: usize,    // Original index within the plugin's list.
}

// Manages a collection of all plugins loaded by the server.
pub struct PluginManager {
    // A list of all successfully loaded plugins.
    pub plugins: Vec<Arc<LoadedPlugin>>,
    // For quick lookup of a plugin by its ID (e.g., filename).
    plugins_by_id: HashMap<String, Arc<LoadedPlugin>>,
}

impl PluginManager {
    // Constructor that scans a directory, loads all valid plugins,
    // and initializes the PluginManager.
    // This function is `unsafe` because it calls C FFI functions via `libloading`.
    pub unsafe fn new(plugins_dir_path_str: &str) -> Result<Self, AppError> {
        let mut loaded_plugins_vec = Vec::new();
        let mut plugins_map = HashMap::new();
        let plugins_dir_path = Path::new(plugins_dir_path_str);

        if !plugins_dir_path.is_dir() {
            return Err(AppError::PluginLoadError(format!(
                "Plugins directory not found or is not a directory: '{}'",
                plugins_dir_path_str
            )));
        }

        tracing::info!(
            "Scanning for plugins in directory: {}",
            plugins_dir_path_str
        );

        let entries = std::fs::read_dir(plugins_dir_path).map_err(AppError::IoError)?;

        for entry_result in entries {
            let entry = entry_result.map_err(AppError::IoError)?;
            let path = entry.path();

            if !path.is_file() {
                // Skip non-file entries (e.g., directories, symlinks).
                continue;
            }

            if !path
                .file_name()
                .and_then(|s| Some(OsStr::to_str(s)?.starts_with("upsclr-plugin-")))
                .unwrap_or(false)
            {
                // Skip files that don't start with "upsclr-plugin-".
                continue;
            }

            let ext = path.extension().and_then(OsStr::to_str);
            if !matches!(ext, Some("dll") | Some("so") | Some("dylib")) {
                // Skip files that are not shared libraries (DLL, SO, DYLIB).
                continue;
            }

            // Common shared lib extensions
            tracing::debug!("Attempting to load plugin from: {:?}", path);
            match Self::load_plugin_from_path(&path) {
                Ok(loaded_plugin_arc) => {
                    tracing::info!(
                        "Successfully loaded plugin '{}' (ID: {}) from {:?}",
                        loaded_plugin_arc.plugin_info.name,
                        loaded_plugin_arc.id,
                        path
                    );
                    plugins_map.insert(loaded_plugin_arc.id.clone(), loaded_plugin_arc.clone());
                    loaded_plugins_vec.push(loaded_plugin_arc);
                }
                Err(e) => {
                    // Log error and continue to try loading other plugins.
                    tracing::error!("Failed to load plugin from {:?}: {}", path, e);
                    // Depending on policy, one bad plugin could halt server startup.
                    // Here, we just skip it.
                }
            }
        }

        if loaded_plugins_vec.is_empty() {
            tracing::warn!(
                "No plugins were successfully loaded from directory: {}. The server might not have any upscaling capabilities.",
                plugins_dir_path_str
            );
        }

        Ok(PluginManager {
            plugins: loaded_plugins_vec,
            plugins_by_id: plugins_map,
        })
    }

    // Helper function to load a single plugin from a given filesystem path.
    // This is `unsafe` due to FFI interactions.
    unsafe fn load_plugin_from_path(path: &Path) -> Result<Arc<LoadedPlugin>, String> {
        // Load the shared library.
        let lib = Library::new(path)
            .map_err(|e| format!("Failed to load shared library from {:?}: {}", path, e))?;

        // Macro to simplify symbol loading and error mapping.
        macro_rules! get_symbol {
            ($lib:expr, $name:expr) => {
                $lib.get($name).map_err(|e| {
                    format!(
                        "Failed to load symbol '{}' from {:?}: {}",
                        String::from_utf8_lossy($name),
                        path,
                        e
                    )
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
        let init_result = initialize_fn();
        if init_result != plugin_ffi::UpsclrErrorCode::Success {
            return Err(format!(
                "Plugin at {:?} failed to initialize with error code: {:?}",
                path, init_result
            ));
        }

        // Call `upsclr_plugin_get_info` to retrieve basic plugin metadata.
        let c_plugin_info_ptr = get_info_fn();
        if c_plugin_info_ptr.is_null() {
            return Err(format!(
                "Plugin at {:?} returned NULL from upsclr_plugin_get_info",
                path
            ));
        }
        let c_plugin_info = &*c_plugin_info_ptr; // Unsafe dereference. Valid if plugin API is correct.

        let plugin_id = path
            .file_name()
            .unwrap_or_else(|| OsStr::new("unknown_plugin"))
            .to_string_lossy()
            .into_owned();

        let plugin_info_rs = PluginInfo {
            id: plugin_id.clone(),
            name: Self::c_str_to_rust_string(c_plugin_info.name)
                .map_err(|e| format!("Invalid plugin name from {:?}: {}", path, e))?,
            version: Self::c_str_to_rust_string(c_plugin_info.version)
                .map_err(|e| format!("Invalid plugin version from {:?}: {}", path, e))?,
            description: Self::c_str_to_rust_string(c_plugin_info.description)
                .map_err(|e| format!("Invalid plugin description from {:?}: {}", path, e))?,
        };

        // Retrieve information for each engine provided by the plugin.
        let mut engines_rs = Vec::new();
        let num_engines = count_engines_fn();
        for i in 0..num_engines {
            let c_engine_info_ptr = get_engine_info_fn(i);
            if c_engine_info_ptr.is_null() {
                return Err(format!(
                    "Plugin at {:?} returned NULL from upsclr_plugin_get_engine_info for engine index {}",
                    path, i
                ));
            }
            let c_engine_info = &*c_engine_info_ptr; // Unsafe dereference.
            engines_rs.push(EngineInfo {
                name: Self::c_str_to_rust_string(c_engine_info.name).map_err(|e| {
                    format!("Invalid engine name (idx {}) from {:?}: {}", i, path, e)
                })?,
                description: Self::c_str_to_rust_string(c_engine_info.description).map_err(
                    |e| {
                        format!(
                            "Invalid engine description (idx {}) from {:?}: {}",
                            i, path, e
                        )
                    },
                )?,
                version: Self::c_str_to_rust_string(c_engine_info.version).map_err(|e| {
                    format!("Invalid engine version (idx {}) from {:?}: {}", i, path, e)
                })?,
                config_schema: serde_json::from_str(
                    &Self::c_str_to_rust_string(c_engine_info.config_json_schema).map_err(|e| {
                        format!("Invalid engine schema (idx {}) from {:?}: {}", i, path, e)
                    })?,
                )
                .map_err(|e| format!("Invalid JSON schema (idx {}) from {:?}: {:?}", i, path, e))?,
                plugin_id: plugin_id.clone(),
                engine_index_in_plugin: i,
            });
        }

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
            id: plugin_id.clone(),
            upsclr_plugin_initialize: initialize_fn_ptr,
            upsclr_plugin_shutdown: shutdown_fn_ptr,
        });

        // Construct and return the LoadedPlugin struct, now populated with function pointers and metadata.
        Ok(Arc::new(LoadedPlugin {
            _lib: plugin_lib_arc,
            id: plugin_id,
            path: path.to_path_buf(),
            plugin_info: plugin_info_rs,
            engines_info: engines_rs,
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
        }))
    }

    // Helper function to safely convert a C-style string (UTF-8 encoded) to a Rust String.
    // Returns a Result to handle potential null pointers or invalid UTF-8.
    pub(crate) unsafe fn c_str_to_rust_string(
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

    // Finds a loaded plugin by its unique ID (typically its filename).
    pub fn find_plugin_by_id(&self, id: &str) -> Option<Arc<LoadedPlugin>> {
        self.plugins_by_id.get(id).cloned()
    }
}

// This file contains Rust definitions for the C plugin API,
// based on the provided upsclr_plugin.h header.

use std::os::raw::{c_char, c_void};

// For FFI, Rust's *const c_char (which is *const i8) is appropriate for C 'char *' and 'char8_t *'.
#[allow(non_camel_case_types)]
pub type c_char8_t = c_char;

#[repr(C)]
#[derive(Debug)]
pub struct UpsclrPluginInfo {
    pub name: *const c_char8_t,
    pub version: *const c_char8_t,
    pub description: *const c_char8_t,
}

#[repr(C)]
#[derive(Debug)]
pub struct UpsclrEngineInfo {
    pub name: *const c_char8_t,
    pub description: *const c_char8_t,
    pub version: *const c_char8_t,
    pub config_json_schema: *const c_char8_t,
}

// Opaque pointer type for engine instances.
// The actual definition is provided in the plugin's implementation.
#[repr(C)]
pub struct UpsclrEngineInstance(c_void); // Using c_void for an opaque struct

#[repr(C)]
#[derive(Debug)]
pub struct UpsclrEngineConfigValidationResult {
    pub is_valid: bool,
    pub error_count: usize,
    pub warning_count: usize,
    pub warning_messages: *const *const c_char8_t, // Array of C strings (UTF-8)
    pub error_messages: *const *const c_char8_t,   // Array of C strings (UTF-8)
}

#[repr(C)]
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
#[non_exhaustive]
pub enum UpsclrErrorCode {
    Success = 0,                  // UPSCLR_SUCCESS
    PluginNotInitialized = 1,     // UPSCLR_ERROR_PLUGIN_NOT_INITIALIZED
    PluginAlreadyInitialized = 2, // UPSCLR_ERROR_PLUGIN_ALREADY_INITIALIZED
    PluginAlreadyDestroyed = 3,   // UPSCLR_ERROR_PLUGIN_ALREADY_DESTROYED
    InvalidArgument = 4,          // UPSCLR_ERROR_INVALID_ARGUMENT
    EngineNotFound = 5,           // UPSCLR_ERROR_ENGINE_NOT_FOUND
    PreloadFailed = 6,            // UPSCLR_ERROR_PRELOAD_FAILED
    UpscaleFailed = 7,            // UPSCLR_ERROR_UPSCALE_FAILED
    Other = 9999,                 // UPSCLR_ERROR_OTHER
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpsclrColorFormat {
    Rgb = 0, // UPSCLR_COLOR_FORMAT_RGB (Red, Green, Blue)
    Bgr = 1, // UPSCLR_COLOR_FORMAT_BGR (Blue, Green, Red)
}

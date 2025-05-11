// This file contains Rust definitions for the C plugin API,
// based on the provided upsclr_plugin.h header.

use std::os::raw::{c_char, c_void};

// char8_t is specified as UTF-8. In C, this is typically 'char'.
// For FFI, Rust's *const c_char (which is *const i8) is appropriate for C 'char *'.
pub type c_char8_t = c_char;

#[repr(C)]
#[derive(Debug, Clone)] // Added Clone for potential copying if needed (careful with pointers)
pub struct UpsclrPluginInfo {
    pub name: *const c_char8_t,
    pub version: *const c_char8_t,
    pub description: *const c_char8_t,
}

#[repr(C)]
#[derive(Debug, Clone)] // Added Clone
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
#[derive(Debug)] // Removed Clone as it contains pointers to arrays managed by C code
pub struct UpsclrEngineConfigValidationResult {
    pub is_valid: bool,
    pub error_count: usize,
    pub warning_count: usize,
    pub warning_messages: *const *const c_char8_t, // Array of C strings (UTF-8)
    pub error_messages: *const *const c_char8_t,   // Array of C strings (UTF-8)
}

#[repr(C)]
#[derive(Debug, PartialEq, Eq, Clone, Copy)] // Make it fully usable in Rust comparisons
#[non_exhaustive]
pub enum UpsclrErrorCode {
    Success = 0,         // UPSCLR_SUCCESS
    InvalidArgument = 1, // UPSCLR_ERROR_INVALID_ARGUMENT
    EngineNotFound = 2,  // UPSCLR_ERROR_ENGINE_NOT_FOUND
    UpscaleFailed = 3,   // UPSCLR_ERROR_UPSCALE_FAILED
    Other = 9999,        // UPSCLR_ERROR_OTHER
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)] // Make it fully usable in Rust
pub enum UpsclrColorFormat {
    Rgb = 0, // UPSCLR_COLOR_FORMAT_RGB (Red, Green, Blue)
    Bgr = 1, // UPSCLR_COLOR_FORMAT_BGR (Blue, Green, Red)
}

// Note: Function pointer type aliases (e.g., UpsclrPluginGetInfoFn)
// are not strictly necessary here as libloading::Symbol will store symbols
// with their full types. They can be useful for documentation or type casting.

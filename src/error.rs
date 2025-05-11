// src/error.rs
// Defines custom error types for the application and their
// conversion into HTTP responses.

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json; // For creating JSON error bodies

#[derive(Debug)]
pub enum AppError {
    // Errors related to plugin loading and management
    PluginLoadError(String),
    PluginNotFound(String),
    EngineNotFound(String),

    // Errors related to instance management
    InstanceNotFound,
    InstanceCreationFailed(String), // More specific than generic PluginOperationFailed

    // Errors from plugin operations (preload, upscale)
    PluginOperationFailed { operation: String, details: String },

    // Errors related to request processing
    BadRequest(String), // General bad request (e.g., invalid parameters)
    MultipartError(axum::extract::multipart::MultipartError),
    UnsupportedMediaType(String), // Unsupported media type (e.g., Content-Type)
    NotAcceptable(String),        // Unsupported Accept header
    UnprocessableContent(String), // Unprocessable content (e.g., image too large)
    ImageProcessingError(String), // Errors during image decoding/encoding

    // General I/O or internal errors
    IoError(std::io::Error),
    CStringError(std::ffi::NulError), // Error converting Rust string to CString
    InternalServerError(String),      // For miscellaneous server issues
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, error_message, error_code_str) = match self {
            AppError::PluginLoadError(s) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("A critical error occurred while loading plugins: {}", s),
                "PLUGIN_LOAD_ERROR",
            ),
            AppError::PluginNotFound(s) => (
                StatusCode::BAD_REQUEST,
                format!("The requested plugin was not found: {}", s),
                "PLUGIN_NOT_FOUND",
            ),
            AppError::EngineNotFound(s) => (
                StatusCode::BAD_REQUEST,
                format!("The requested engine was not found: {}", s),
                "ENGINE_NOT_FOUND",
            ),
            AppError::InstanceNotFound => (
                StatusCode::NOT_FOUND,
                "The requested engine instance was not found.".to_string(),
                "INSTANCE_NOT_FOUND",
            ),
            AppError::InstanceCreationFailed(s) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to create engine instance: {}", s),
                "INSTANCE_CREATION_FAILED",
            ),
            AppError::PluginOperationFailed { operation, details } => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Plugin operation '{}' failed: {}", operation, details),
                "PLUGIN_OPERATION_FAILED",
            ),
            AppError::BadRequest(s) => (StatusCode::BAD_REQUEST, s, "BAD_REQUEST"),
            AppError::MultipartError(e) => (
                StatusCode::BAD_REQUEST,
                format!("Invalid multipart request: {}", e),
                "MULTIPART_ERROR",
            ),
            AppError::UnsupportedMediaType(s) => (
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                format!("Unsupported media type to read: {}", s),
                "UNSUPPORTED_MEDIA_TYPE",
            ),
            AppError::NotAcceptable(s) => (
                StatusCode::NOT_ACCEPTABLE,
                format!("Unsupported media type to write: {}", s),
                "NOT_ACCEPTABLE",
            ),
            AppError::UnprocessableContent(s) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("Unprocessable content: {}", s),
                "UNPROCESSABLE_CONTENT",
            ),
            AppError::ImageProcessingError(s) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("Image processing error: {}", s),
                "IMAGE_PROCESSING_ERROR",
            ),
            AppError::IoError(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("An I/O error occurred: {}", e),
                "IO_ERROR",
            ),
            AppError::CStringError(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Internal server error preparing data for plugin: {}", e),
                "CSTRING_CONVERSION_ERROR",
            ),
            AppError::InternalServerError(s) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                s,
                "INTERNAL_SERVER_ERROR",
            ),
        };

        let body = Json(json!({
            "error": {
                "code": error_code_str,
                "message": error_message,
            }
        }));
        (status, body).into_response()
    }
}

// Implement From trait for common error types to simplify error handling in handlers.
impl From<std::io::Error> for AppError {
    fn from(err: std::io::Error) -> Self {
        AppError::IoError(err)
    }
}

impl From<axum::extract::multipart::MultipartError> for AppError {
    fn from(err: axum::extract::multipart::MultipartError) -> Self {
        AppError::MultipartError(err)
    }
}

impl From<std::ffi::NulError> for AppError {
    fn from(err: std::ffi::NulError) -> Self {
        AppError::CStringError(err)
    }
}

impl From<image::ImageError> for AppError {
    fn from(err: image::ImageError) -> Self {
        AppError::ImageProcessingError(err.to_string())
    }
}

// Helper for locking mutexes, converting PoisonError to AppError
pub fn lock_mutex_app_error<'a, T>(
    mutex: &'a std::sync::Mutex<T>,
    operation_name: &'static str,
) -> Result<std::sync::MutexGuard<'a, T>, AppError> {
    mutex.lock().map_err(|e| {
        AppError::InternalServerError(format!(
            "Failed to acquire lock for {}: {}",
            operation_name, e
        ))
    })
}

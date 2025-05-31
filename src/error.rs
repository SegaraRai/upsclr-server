// src/error.rs
// Defines custom error types for the application and their
// conversion into HTTP responses.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
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

impl<T> From<std::sync::PoisonError<T>> for AppError {
    fn from(err: std::sync::PoisonError<T>) -> Self {
        AppError::InternalServerError(format!("Mutex lock poisoned: {}", err))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{http::StatusCode, response::Response};
    use serde_json::Value;

    async fn extract_error_body(response: Response) -> Value {
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    #[tokio::test]
    async fn test_plugin_not_found_error_response() {
        let error = AppError::PluginNotFound("test_plugin".to_string());
        let response = error.into_response();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let body = extract_error_body(response).await;
        assert_eq!(body["error"]["code"], "PLUGIN_NOT_FOUND");
        assert!(body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("test_plugin"));
    }

    #[tokio::test]
    async fn test_instance_not_found_error_response() {
        let error = AppError::InstanceNotFound;
        let response = error.into_response();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let body = extract_error_body(response).await;
        assert_eq!(body["error"]["code"], "INSTANCE_NOT_FOUND");
        assert_eq!(
            body["error"]["message"],
            "The requested engine instance was not found."
        );
    }

    #[tokio::test]
    async fn test_bad_request_error_response() {
        let error = AppError::BadRequest("Invalid parameter".to_string());
        let response = error.into_response();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let body = extract_error_body(response).await;
        assert_eq!(body["error"]["code"], "BAD_REQUEST");
        assert_eq!(body["error"]["message"], "Invalid parameter");
    }

    #[tokio::test]
    async fn test_plugin_operation_failed_error_response() {
        let error = AppError::PluginOperationFailed {
            operation: "upscale".to_string(),
            details: "Out of memory".to_string(),
        };
        let response = error.into_response();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let body = extract_error_body(response).await;
        assert_eq!(body["error"]["code"], "PLUGIN_OPERATION_FAILED");
        assert!(body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("upscale"));
        assert!(body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Out of memory"));
    }

    #[tokio::test]
    async fn test_unsupported_media_type_error_response() {
        let error = AppError::UnsupportedMediaType("image/bmp".to_string());
        let response = error.into_response();

        assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

        let body = extract_error_body(response).await;
        assert_eq!(body["error"]["code"], "UNSUPPORTED_MEDIA_TYPE");
        assert!(body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("image/bmp"));
    }

    #[test]
    fn test_from_io_error() {
        let io_error = std::io::Error::new(std::io::ErrorKind::NotFound, "File not found");
        let app_error: AppError = io_error.into();

        match app_error {
            AppError::IoError(_) => (),
            _ => panic!("Expected IoError variant"),
        }
    }

    #[test]
    fn test_from_cstring_error() {
        let cstring_error = std::ffi::CString::new("test\0with\0nulls").unwrap_err();
        let app_error: AppError = cstring_error.into();

        match app_error {
            AppError::CStringError(_) => (),
            _ => panic!("Expected CStringError variant"),
        }
    }

    #[test]
    fn test_from_image_error() {
        let image_error =
            image::ImageError::Unsupported(image::error::UnsupportedError::from_format_and_kind(
                image::error::ImageFormatHint::Unknown,
                image::error::UnsupportedErrorKind::Format(image::error::ImageFormatHint::Unknown),
            ));
        let app_error: AppError = image_error.into();

        match app_error {
            AppError::ImageProcessingError(_) => (),
            _ => panic!("Expected ImageProcessingError variant"),
        }
    }
}

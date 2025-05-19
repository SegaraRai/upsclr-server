// Error types for the API server

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use uuid::Uuid;

/// API server error types
#[derive(Debug)]
pub enum ApiError {
    BadRequest(String),
    Unauthorized(String),
    Forbidden(String),
    NotFound(String),
    MethodNotAllowed(String),
    NotAcceptable(String),
    RequestTimeout(String),
    Conflict(String),
    Gone(String),
    LengthRequired(String),
    PayloadTooLarge(String),
    UnsupportedMediaType(String),
    UnprocessableEntity(String),
    TooManyRequests(String),
    InternalServerError(String),
    NotImplemented(String),
    BadGateway(String),
    ServiceUnavailable(String),
    GatewayTimeout(String),

    // Application-specific errors
    PluginNotFound(String),
    PluginLoadError(String),
    EngineNotFound(String),
    InstanceNotFound(Uuid),
    ImageProcessingError(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, error_message) = match self {
            Self::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            Self::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, msg),
            Self::Forbidden(msg) => (StatusCode::FORBIDDEN, msg),
            Self::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
            Self::MethodNotAllowed(msg) => (StatusCode::METHOD_NOT_ALLOWED, msg),
            Self::NotAcceptable(msg) => (StatusCode::NOT_ACCEPTABLE, msg),
            Self::RequestTimeout(msg) => (StatusCode::REQUEST_TIMEOUT, msg),
            Self::Conflict(msg) => (StatusCode::CONFLICT, msg),
            Self::Gone(msg) => (StatusCode::GONE, msg),
            Self::LengthRequired(msg) => (StatusCode::LENGTH_REQUIRED, msg),
            Self::PayloadTooLarge(msg) => (StatusCode::PAYLOAD_TOO_LARGE, msg),
            Self::UnsupportedMediaType(msg) => (StatusCode::UNSUPPORTED_MEDIA_TYPE, msg),
            Self::UnprocessableEntity(msg) => (StatusCode::UNPROCESSABLE_ENTITY, msg),
            Self::TooManyRequests(msg) => (StatusCode::TOO_MANY_REQUESTS, msg),
            Self::InternalServerError(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
            Self::NotImplemented(msg) => (StatusCode::NOT_IMPLEMENTED, msg),
            Self::BadGateway(msg) => (StatusCode::BAD_GATEWAY, msg),
            Self::ServiceUnavailable(msg) => (StatusCode::SERVICE_UNAVAILABLE, msg),
            Self::GatewayTimeout(msg) => (StatusCode::GATEWAY_TIMEOUT, msg),

            // Map application-specific errors to appropriate HTTP status codes
            Self::PluginNotFound(msg) => (StatusCode::NOT_FOUND, msg),
            Self::PluginLoadError(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
            Self::EngineNotFound(msg) => (StatusCode::NOT_FOUND, msg),
            Self::InstanceNotFound(uuid) => (
                StatusCode::NOT_FOUND,
                format!("Instance with ID {} not found", uuid),
            ),
            Self::ImageProcessingError(msg) => (StatusCode::BAD_REQUEST, msg),
        };

        let body = Json(json!({
            "error": {
                "status": status.as_u16(),
                "message": error_message,
            }
        }));

        (status, body).into_response()
    }
}

// Implement From<PluginHostError> for ApiError
impl From<crate::models::PluginHostError> for ApiError {
    fn from(error: crate::models::PluginHostError) -> Self {
        match error {
            crate::models::PluginHostError::PluginNotFound(msg) => Self::PluginNotFound(msg),
            crate::models::PluginHostError::PluginLoadError(msg) => Self::PluginLoadError(msg),
            crate::models::PluginHostError::EngineNotFound(msg) => Self::EngineNotFound(msg),
            crate::models::PluginHostError::EngineCreationFailed(msg) => Self::BadRequest(msg),
            crate::models::PluginHostError::EngineInstanceNotFound(uuid) => Self::InstanceNotFound(uuid),
            crate::models::PluginHostError::OperationFailed(msg) => Self::InternalServerError(msg),
            crate::models::PluginHostError::InvalidParameter(msg) => Self::BadRequest(msg),
            crate::models::PluginHostError::UnsupportedOperation(msg) => Self::NotImplemented(msg),
            crate::models::PluginHostError::ResourceLimitExceeded(msg) => Self::TooManyRequests(msg),
            crate::models::PluginHostError::Timeout(msg) => Self::RequestTimeout(msg),
            crate::models::PluginHostError::InternalError(msg) => Self::InternalServerError(msg),
        }
    }
}

use axum::{
    body,
    extract::{FromRequest, Multipart, Request},
    http::header,
};
use tracing::{debug, warn};

use super::error::ApiError;

pub async fn extract_request_image(
    request: Request,
) -> Result<(Vec<u8>, Option<String>), ApiError> {
    // Get the content type from the request headers
    let content_type = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    // Extract image data based on content type
    if content_type.starts_with("multipart/form-data") {
        extract_multipart_image(request).await
    } else {
        extract_direct_image(request, &content_type).await
    }
}

// Helper function to extract image data from a multipart request
async fn extract_multipart_image(request: Request) -> Result<(Vec<u8>, Option<String>), ApiError> {
    // Convert Request to Multipart
    let mut multipart = Multipart::from_request(request, &())
        .await
        .map_err(|e| ApiError::BadRequest(format!("Failed to process multipart request: {}", e)))?;

    let mut file_data_opt: Option<Vec<u8>> = None;
    let mut content_type_opt: Option<String> = None;
    let mut ignored_fields = 0;

    // Loop through all fields to find "file" and ignore others
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::BadRequest(format!("Failed to process multipart field: {}", e)))?
    {
        if field.name() == Some("file") {
            if file_data_opt.is_some() {
                // Found a second "file" field
                warn!("Multiple 'file' fields found in multipart request, using the last one");
            }

            let content_type_str = field.content_type().map(str::to_string);
            debug!("Received file with content type: {:?}", content_type_str);

            let data = field
                .bytes()
                .await
                .map_err(|e| ApiError::BadRequest(format!("Failed to read file data: {}", e)))?
                .to_vec();

            if data.is_empty() {
                return Err(ApiError::BadRequest(
                    "Uploaded 'file' field is empty.".to_string(),
                ));
            }

            file_data_opt = Some(data);
            content_type_opt = content_type_str;
        } else {
            let field_name = field.name().unwrap_or("unnamed").to_string();
            debug!("Ignoring multipart field: {}", field_name);
            ignored_fields += 1;
        }
    }

    if ignored_fields > 0 {
        debug!(
            "Ignored {} non-file fields in multipart request",
            ignored_fields
        );
    }

    match file_data_opt {
        Some(data) => Ok((data, content_type_opt)),
        None => Err(ApiError::BadRequest(
            "Missing 'file' field in multipart request.".to_string(),
        )),
    }
}

// Helper function to extract image data from a direct (non-multipart) request
async fn extract_direct_image(
    request: Request,
    content_type: &str,
) -> Result<(Vec<u8>, Option<String>), ApiError> {
    // Validate that Content-Type is a supported image format
    if !content_type.starts_with("image/") && !content_type.starts_with("application/octet-stream")
    {
        return Err(ApiError::UnsupportedMediaType(format!(
            "Content-Type '{}' is not supported. Expected image/*, multipart/form-data, or application/octet-stream.",
            content_type
        )));
    }

    // Extract the body as bytes
    let body = request.into_body();
    let bytes = body::to_bytes(body, usize::MAX)
        .await
        .map_err(|e| ApiError::BadRequest(format!("Failed to read request body: {}", e)))?;

    if bytes.is_empty() {
        return Err(ApiError::BadRequest("Request body is empty.".to_string()));
    }

    // Return the bytes and content type
    Ok((bytes.to_vec(), Some(content_type.to_string())))
}

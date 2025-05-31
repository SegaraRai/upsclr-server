// Contains the Axum handler functions for each API endpoint.
// These handlers process requests, interact with managers, and generate responses.
// Includes handlers that use the combined state tuple (SharedPluginManager, SharedInstanceManager).

use crate::{
    error::AppError, headers, instance_manager::InstanceManager, models::*, plugin_ffi,
    plugin_manager::PluginManager,
};
use axum::{
    Json,
    body::{self},
    extract::{FromRequest, Multipart, Path, Query, Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use axum_extra::TypedHeader;
use std::sync::{Arc, RwLock};
use uuid::Uuid;

// --- Shared State Type Aliases (for convenience in handler signatures) ---
#[allow(unused)]
pub type SharedPluginManager = Arc<RwLock<PluginManager>>;
#[allow(unused)]
pub type SharedInstanceManager = Arc<RwLock<InstanceManager>>;

pub const MAX_IMAGE_SIZE_BYTES: usize = 100 * 1024 * 1024; // Example: 100MB limit for input image.

// --- Security Validation Middleware ---

/// Configuration for request security validation
#[derive(Debug, Clone)]
pub struct SecurityConfig {
    pub require_user_agent_prefix: bool,
    pub require_upsclr_request_header: bool,
}

/// Middleware to validate request security headers (User-Agent and Upsclr-Request)
pub async fn validate_request_security(
    config: axum::Extension<SecurityConfig>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Result<Response, AppError> {
    let headers = request.headers();

    // Validate User-Agent header if configured
    if config.require_user_agent_prefix {
        if let Some(user_agent_header) = headers.get(axum::http::header::USER_AGENT) {
            let user_agent_value = user_agent_header
                .to_str()
                .map_err(|_| AppError::Forbidden("Invalid User-Agent header format".to_string()))?;
            if !user_agent_value.starts_with("Upsclr/") && !user_agent_value.starts_with("Upsclr-")
            {
                return Err(AppError::Forbidden(format!(
                    "Invalid User-Agent header: expected to start with 'Upsclr/' or 'Upsclr-', got '{}'",
                    user_agent_value
                )));
            }
        } else {
            return Err(AppError::Forbidden(
                "Missing required User-Agent header".to_string(),
            ));
        }
    }

    // Validate Upsclr-Request header if configured
    if config.require_upsclr_request_header {
        if let Some(upsclr_request_header) = headers.get("Upsclr-Request") {
            let upsclr_request_value = upsclr_request_header.to_str().map_err(|_| {
                AppError::Forbidden("Invalid Upsclr-Request header format".to_string())
            })?;
            if upsclr_request_value.is_empty() {
                return Err(AppError::Forbidden(
                    "Upsclr-Request header must not be empty".to_string(),
                ));
            }
        } else {
            return Err(AppError::Forbidden(
                "Missing required Upsclr-Request header".to_string(),
            ));
        }
    }

    Ok(next.run(request).await)
}

// OutputFormat enum for handling different image output formats
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum OutputFormat {
    Png {
        compression: u8,
    },
    Jpeg {
        quality: u8,
    },
    Raw {
        color_format: plugin_ffi::UpsclrColorFormat,
    },
}

impl TryFrom<&mime::Mime> for OutputFormat {
    type Error = ();

    fn try_from(value: &mime::Mime) -> Result<Self, Self::Error> {
        if value.type_() == mime::IMAGE {
            match value.subtype().as_str() {
                "png" => {
                    let compression = value
                        .get_param("compression")
                        .and_then(|c| c.as_str().parse::<u8>().ok())
                        .unwrap_or(6);
                    Ok(OutputFormat::Png { compression })
                }
                "jpeg" => {
                    let quality = value
                        .get_param("quality")
                        .and_then(|q| q.as_str().parse::<u8>().ok())
                        .unwrap_or(85);
                    Ok(OutputFormat::Jpeg { quality })
                }
                "x-raw-bitmap" => {
                    let color_format = value.get_param("format").map_or(
                        plugin_ffi::UpsclrColorFormat::Rgb,
                        |cf| {
                            let lower_str = cf.as_str().to_ascii_lowercase();
                            if lower_str == "bgr" || lower_str == "bgra" {
                                plugin_ffi::UpsclrColorFormat::Bgr
                            } else {
                                plugin_ffi::UpsclrColorFormat::Rgb
                            }
                        },
                    );
                    Ok(OutputFormat::Raw { color_format })
                }
                _ => Err(()),
            }
        } else {
            Err(())
        }
    }
}

impl From<OutputFormat> for plugin_ffi::UpsclrColorFormat {
    fn from(value: OutputFormat) -> Self {
        match value {
            OutputFormat::Png { .. } => plugin_ffi::UpsclrColorFormat::Rgb,
            OutputFormat::Jpeg { .. } => plugin_ffi::UpsclrColorFormat::Rgb,
            OutputFormat::Raw { color_format } => color_format,
        }
    }
}

// Helper function to decode various input image formats.
fn decode_input_image(
    file_data: &[u8],
    content_type_str: Option<&str>,
) -> Result<(Vec<u8>, u32, u32, u8, plugin_ffi::UpsclrColorFormat), AppError> {
    use byteorder::{ByteOrder, LittleEndian};

    let media_type = content_type_str.map(|s| s[0..s.find(';').unwrap_or(s.len())].trim());

    match media_type {
        Some("image/jpeg") | Some("image/png") | Some("image/webp") | None => {
            // Determine image format from content type string.
            let img_format_hint = match media_type {
                Some("image/jpeg") => Some(image::ImageFormat::Jpeg),
                Some("image/png") => Some(image::ImageFormat::Png),
                Some("image/webp") => Some(image::ImageFormat::WebP),
                _ => None,
            };

            let dyn_img = if let Some(format) = img_format_hint {
                image::load_from_memory_with_format(file_data, format).map_err(|e| {
                    AppError::ImageProcessingError(format!(
                        "Failed to decode image (format: {:?}): {}",
                        format, e
                    ))
                })?
            } else {
                image::load_from_memory(file_data).map_err(|e| {
                    AppError::ImageProcessingError(format!(
                        "Failed to auto-detect and decode image: {}",
                        e
                    ))
                })?
            };

            let width = dyn_img.width();
            let height = dyn_img.height();
            let channels = if dyn_img.color().has_alpha() { 4 } else { 3 };

            let data_vec = if channels == 3 {
                dyn_img.to_rgb8().into_raw()
            } else {
                dyn_img.to_rgba8().into_raw()
            };
            Ok((
                data_vec,
                width,
                height,
                channels,
                plugin_ffi::UpsclrColorFormat::Rgb,
            ))
        }
        Some("image/x-raw-bitmap") => {
            // Parse the 16-byte header for custom "image/x-raw-bitmap" format.
            // Header: ["R"]["B"](2B), ColorFormat(1B), Channels(1B), DataSize(4B LE), Width(4B LE), Height(4B LE)
            if file_data.len() < 16 {
                return Err(AppError::ImageProcessingError(
                    "x-raw-bitmap data is too short for its 16-byte header.".to_string(),
                ));
            }

            if &file_data[0..2] != b"RB" {
                // Magic bytes check.
                return Err(AppError::ImageProcessingError(
                    "Invalid magic bytes for x-raw-bitmap. Expected 'RB'.".to_string(),
                ));
            }

            let header_color_format_byte = file_data[2]; // 0x01 for RGB/RGBA, 0x02 for BGR/BGRA
            let header_channels_byte = file_data[3]; // 3 for RGB/BGR, 4 for RGBA/BGRA
            let data_size_from_header = LittleEndian::read_u32(&file_data[4..8]); // Size of pixel data *after* header
            let width = LittleEndian::read_u32(&file_data[8..12]);
            let height = LittleEndian::read_u32(&file_data[12..16]);

            if width == 0 || height == 0 {
                return Err(AppError::ImageProcessingError(
                    "x-raw-bitmap header indicates zero width or height.".to_string(),
                ));
            }
            if header_channels_byte != 3 && header_channels_byte != 4 {
                return Err(AppError::ImageProcessingError(format!(
                    "x-raw-bitmap header specifies an unsupported channel count: {}. Expected 3 or 4.",
                    header_channels_byte
                )));
            }

            // Map header's ColorFormat byte to the plugin's plugin_ffi::UpsclrColorFormat enum.
            let plugin_color_format = match header_color_format_byte {
                0x01 => plugin_ffi::UpsclrColorFormat::Rgb, // Covers RGB and RGBA (plugin gets channels separately)
                0x02 => plugin_ffi::UpsclrColorFormat::Bgr, // Covers BGR and BGRA
                invalid_byte => {
                    return Err(AppError::ImageProcessingError(format!(
                        "Invalid ColorFormat byte (0x{:02X}) in x-raw-bitmap header. Expected 0x01 or 0x02.",
                        invalid_byte
                    )));
                }
            };

            // Validate DataSize from header against calculated size and actual payload size.
            let calculated_pixel_data_size = (width as usize)
                .saturating_mul(height as usize)
                .saturating_mul(header_channels_byte as usize);

            if data_size_from_header as usize != calculated_pixel_data_size {
                return Err(AppError::ImageProcessingError(format!(
                    "x-raw-bitmap DataSize in header ({}) does not match calculated size ({}) from WxHxC.",
                    data_size_from_header, calculated_pixel_data_size
                )));
            }

            let expected_total_size = 16 + data_size_from_header as usize;
            if file_data.len() != expected_total_size {
                return Err(AppError::ImageProcessingError(format!(
                    "x-raw-bitmap total file size ({}) does not match expected size based on header ({}).",
                    file_data.len(),
                    expected_total_size
                )));
            }

            // Extract pixel data (skip the 16-byte header).
            let data_vec = file_data[16..].to_vec();

            Ok((
                data_vec,
                width,
                height,
                header_channels_byte,
                plugin_color_format,
            ))
        }
        Some(unknown_content_type) => Err(AppError::UnsupportedMediaType(format!(
            "Content type '{}' is not supported. Expected 'image/jpeg', 'image/png', 'image/webp', or 'image/x-raw-bitmap'.",
            unknown_content_type
        ))),
    }
}

// Helper function to encode output image in various formats
fn encode_output_image(
    raw_pixel_data: &[u8],
    width: u32,
    height: u32,
    channels: u8,
    plugin_output_format: OutputFormat,
) -> Result<Response, AppError> {
    use image::ImageFormat;
    use std::io::Cursor;

    match plugin_output_format {
        OutputFormat::Png { compression: _ } => {
            tracing::debug!("Encoding output as PNG.");

            let color_type_for_image_crate = match channels {
                3 => image::ColorType::Rgb8,
                4 => image::ColorType::Rgba8,
                _ => {
                    return Err(AppError::ImageProcessingError(format!(
                        "Unsupported channel count ({}).",
                        channels
                    )));
                }
            };

            let mut buffer = Cursor::new(Vec::new());
            image::write_buffer_with_format(
                &mut buffer,
                raw_pixel_data,
                width,
                height,
                color_type_for_image_crate,
                ImageFormat::Png,
            )
            .map_err(|e| AppError::ImageProcessingError(format!("PNG encoding failed: {}", e)))?;

            Ok((
                [(header::CONTENT_TYPE, "image/png")],
                buffer.into_inner(), // Bytes of the encoded PNG.
            )
                .into_response())
        }
        OutputFormat::Jpeg { quality } => {
            tracing::debug!("Encoding output as JPEG.");

            if channels == 4 {
                // JPEG typically doesn't support an alpha channel.
                // Client should request a format that supports alpha, or server could strip alpha.
                // For now, error out if alpha is present.
                return Err(AppError::ImageProcessingError(
                       "JPEG output for 4-channel data (RGBA/BGRA) is not directly supported. Request 3-channel or PNG/RAW.".to_string()
                   ));
            }

            let mut buffer = Cursor::new(Vec::new());
            let mut encoder =
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buffer, quality);
            encoder
                .encode(
                    raw_pixel_data,
                    width,
                    height,
                    image::ExtendedColorType::Rgb8,
                )
                .map_err(|e| {
                    AppError::ImageProcessingError(format!("JPEG encoding failed: {}", e))
                })?;
            Ok(([(header::CONTENT_TYPE, "image/jpeg")], buffer.into_inner()).into_response())
        }
        OutputFormat::Raw { color_format } => {
            tracing::debug!("Encoding output as x-raw-bitmap.");

            // For x-raw-bitmap, we also need to prepend the 16-byte header.
            let mut raw_bitmap_data = Vec::with_capacity(16 + raw_pixel_data.len());
            raw_bitmap_data.extend_from_slice(b"RB"); // Magic bytes
            raw_bitmap_data.push(match color_format {
                plugin_ffi::UpsclrColorFormat::Rgb => 0x01,
                plugin_ffi::UpsclrColorFormat::Bgr => 0x02,
            }); // ColorFormat byte
            raw_bitmap_data.push(channels); // Channels byte
            let data_size_bytes = (raw_pixel_data.len() as u32).to_le_bytes();
            raw_bitmap_data.extend_from_slice(&data_size_bytes); // DataSize (little-endian)
            raw_bitmap_data.extend_from_slice(&width.to_le_bytes()); // Width (little-endian)
            raw_bitmap_data.extend_from_slice(&height.to_le_bytes()); // Height (little-endian)
            raw_bitmap_data.extend_from_slice(raw_pixel_data); // The pixel data itself

            Ok((
                [(header::CONTENT_TYPE, "image/x-raw-bitmap")],
                raw_bitmap_data,
            )
                .into_response())
        }
    }
}

// Helper function to extract image data from a multipart request
async fn extract_multipart_image(request: Request) -> Result<(Vec<u8>, Option<String>), AppError> {
    // Convert Request to Multipart
    let mut multipart = Multipart::from_request(request, &())
        .await
        .map_err(|e| AppError::BadRequest(format!("Failed to process multipart request: {}", e)))?;

    let mut file_data_opt: Option<Vec<u8>> = None;
    let mut content_type_opt: Option<String> = None;
    let mut ignored_fields = 0;

    // Loop through all fields to find "file" and ignore others
    while let Some(field) = multipart.next_field().await? {
        if field.name() == Some("file") {
            if file_data_opt.is_some() {
                // Found a second "file" field
                tracing::warn!(
                    "Multiple 'file' fields found in multipart request, using the last one"
                );
            }

            let content_type_str = field.content_type().map(str::to_string);
            tracing::debug!("Received file with content type: {:?}", content_type_str);
            let data = field.bytes().await?.to_vec();
            if data.is_empty() {
                return Err(AppError::BadRequest(
                    "Uploaded 'file' field is empty.".to_string(),
                ));
            }
            file_data_opt = Some(data);
            content_type_opt = content_type_str;
        } else {
            let field_name = field.name().unwrap_or("unnamed").to_string();
            tracing::debug!("Ignoring multipart field: {}", field_name);
            ignored_fields += 1;
        }
    }

    if ignored_fields > 0 {
        tracing::debug!(
            "Ignored {} non-file fields in multipart request",
            ignored_fields
        );
    }

    match file_data_opt {
        Some(data) => Ok((data, content_type_opt)),
        None => Err(AppError::BadRequest(
            "Missing 'file' field in multipart request.".to_string(),
        )),
    }
}

// Helper function to extract image data from a direct (non-multipart) request
async fn extract_direct_image(
    request: Request,
    content_type: &str,
) -> Result<(Vec<u8>, Option<String>), AppError> {
    // Validate that Content-Type is a supported image format
    if !content_type.starts_with("image/") && !content_type.starts_with("application/octet-stream")
    {
        return Err(AppError::UnsupportedMediaType(format!(
            "Content-Type '{}' is not supported. Expected image/*, multipart/form-data, or application/octet-stream.",
            content_type
        )));
    }

    // Extract the body as bytes
    let body = request.into_body();
    let bytes = body::to_bytes(body, usize::MAX)
        .await
        .map_err(|e| AppError::BadRequest(format!("Failed to read request body: {}", e)))?;

    if bytes.is_empty() {
        return Err(AppError::BadRequest("Request body is empty.".to_string()));
    }

    // Return the bytes and content type
    Ok((bytes.to_vec(), Some(content_type.to_string())))
}

// --- GET /plugins ---
// Lists all loaded plugins and their available engines.
pub async fn get_plugins(
    State((plugin_manager, _)): State<(SharedPluginManager, SharedInstanceManager)>,
) -> Result<Json<Vec<PluginDescriptionResponse>>, AppError> {
    let descriptions: Vec<PluginDescriptionResponse> = plugin_manager
        .read()?
        .get_plugins()
        .iter()
        .map(|plugin_arc| PluginDescriptionResponse {
            plugin_info: plugin_arc.plugin_info.clone(),
            engines: plugin_arc.engines_info.clone(),
        })
        .collect();
    tracing::debug!("Returning {} plugin descriptions.", descriptions.len());
    Ok(Json(descriptions))
}

// --- GET /instances ---
// Lists all currently active engine instances.
pub async fn list_instances(
    State((_, instance_manager)): State<(SharedPluginManager, SharedInstanceManager)>,
) -> Result<Json<Vec<InstanceInfoForList>>, AppError> {
    let instances_info = instance_manager.read()?.list_instances_info();
    tracing::debug!(
        "Returning {} active instance descriptions.",
        instances_info.len()
    );
    Ok(Json(instances_info))
}

// --- POST /instances ---
// Creates a new engine instance or validates configuration (if dry_run=true).
pub async fn create_instance_handler(
    // Combined state with both managers
    State((plugin_manager, instance_manager)): State<(SharedPluginManager, SharedInstanceManager)>,
    Query(query_params): Query<CreateInstanceQuery>, // Use CreateInstanceQuery from models
    Json(payload): Json<CreateInstanceRequest>,
) -> Result<Json<CreateInstanceResponse>, AppError> {
    let dry_run_active = query_params.dry_run.unwrap_or(false); // Default to false
    tracing::info!(
        "Attempting to create/validate instance for plugin_id: '{}', engine_name: '{}', dry_run: {}",
        payload.plugin_id,
        payload.engine_name,
        dry_run_active
    );

    // Find the specified plugin by its ID.
    let plugin_arc = plugin_manager
        .read()?
        .find_plugin_by_id(&payload.plugin_id)
        .ok_or_else(|| AppError::PluginNotFound(payload.plugin_id.clone()))?;

    // Find the specified engine within the plugin.
    let (engine_index, _engine_info) = plugin_arc
        .engines_info
        .iter()
        .enumerate()
        .find(|(_idx, e_info)| e_info.name == payload.engine_name)
        .ok_or_else(|| {
            AppError::EngineNotFound(format!(
                "Engine '{}' not found in plugin '{}'",
                payload.engine_name, payload.plugin_id
            ))
        })?;

    let config_json_str = payload.config.to_string();
    let config_json_c_str =
        std::ffi::CString::new(config_json_str.as_str()).map_err(AppError::CStringError)?;

    // --- Perform Configuration Validation via Plugin ---
    // This is an unsafe FFI call.
    let validation_c_result_ptr = unsafe {
        (plugin_arc.upsclr_plugin_validate_engine_config)(
            engine_index,
            config_json_c_str.as_ptr(),
            config_json_c_str.as_bytes().len(), // Length without null terminator
        )
    };

    let validation_result_desc = if !validation_c_result_ptr.is_null() {
        let c_result = unsafe { &*validation_c_result_ptr }; // Unsafe dereference
        let mut errors = Vec::new();
        // Check for null pointers before trying to create CStr from them
        if !c_result.error_messages.is_null() {
            for i in 0..c_result.error_count {
                let msg_ptr = unsafe { *c_result.error_messages.add(i) };
                errors.push(
                    unsafe { PluginManager::c_str_to_rust_string(msg_ptr) }.unwrap_or_else(|e| {
                        format!("Failed to parse error message from plugin: {}", e)
                    }),
                );
            }
        }
        let mut warnings = Vec::new();
        if !c_result.warning_messages.is_null() {
            for i in 0..c_result.warning_count {
                let msg_ptr = unsafe { *c_result.warning_messages.add(i) };
                warnings.push(
                    unsafe { PluginManager::c_str_to_rust_string(msg_ptr) }.unwrap_or_else(|e| {
                        format!("Failed to parse warning message from plugin: {}", e)
                    }),
                );
            }
        }
        let desc = ValidationResultDesc {
            is_valid: c_result.is_valid,
            error_count: c_result.error_count,
            error_messages: errors,
            warning_count: c_result.warning_count,
            warning_messages: warnings,
        };
        // Important: Free the validation result memory allocated by the plugin.
        unsafe {
            (plugin_arc.upsclr_plugin_free_validation_result)(validation_c_result_ptr);
        }
        Some(desc)
    } else {
        tracing::warn!(
            "Plugin returned NULL for configuration validation. Assuming valid with no info."
        );
        // If plugin returns NULL, contract might imply "valid with no issues" or an error.
        // Here, we assume it means valid but no specific feedback.
        Some(ValidationResultDesc {
            is_valid: true,
            error_count: 0,
            error_messages: vec![],
            warning_count: 0,
            warning_messages: vec![],
        })
    };

    // If dry_run is active, return validation result without creating an instance.
    if dry_run_active {
        tracing::debug!("Dry run completed for instance creation.");
        return Ok(Json(CreateInstanceResponse {
            instance_id: None,
            validation: validation_result_desc,
        }));
    }

    // If not dry_run, check if the configuration was valid.
    if let Some(ref vr) = validation_result_desc {
        if !vr.is_valid {
            tracing::warn!("Instance configuration invalid: {:?}", vr.error_messages);
            return Err(AppError::BadRequest(format!(
                "Engine configuration is invalid. Errors: {:?}",
                vr.error_messages
            )));
        }
    }
    // If validation_result_desc is None (e.g. plugin returned NULL and we didn't default it),
    // it might be an error or an implicit pass. The current logic defaults it to Some(valid).

    // --- Instance Creation ---
    let active_instance_arc = instance_manager.write()?.create_instance(
        plugin_arc.clone(),
        engine_index,
        &config_json_str,
    )?;

    tracing::info!(
        "Successfully created instance: {}",
        active_instance_arc.uuid
    );
    Ok(Json(CreateInstanceResponse {
        instance_id: Some(active_instance_arc.uuid),
        validation: validation_result_desc, // Include any warnings from validation.
    }))
}

// --- POST /instances/{uuid}/preload?scale=N ---
// Preloads resources for a given engine instance and scale factor.
pub async fn preload_instance(
    State((_, instance_manager)): State<(SharedPluginManager, SharedInstanceManager)>,
    Path(uuid): Path<Uuid>,
    Query(params): Query<ScaleQueryParam>,
) -> Result<StatusCode, AppError> {
    if params.scale <= 1 {
        // Validate scale factor.
        return Err(AppError::BadRequest(
            "Invalid 'scale' parameter. Must be greater than 1.".to_string(),
        ));
    }

    tracing::info!("Preloading instance {} for scale {}", uuid, params.scale);

    // Retrieve the active instance. Arc is cloned to be moved into spawn_blocking.
    let instance_arc = instance_manager
        .read()?
        .get_instance(&uuid)
        .ok_or(AppError::InstanceNotFound)?;

    // Convert the raw pointer into an integer value that can be safely sent between threads
    let instance_ptr_value = instance_arc.instance_ptr as usize;
    let plugin_arc = instance_arc.plugin.clone();
    let scale = params.scale;

    // Preload operation is potentially blocking, so run it in a Tokio blocking task.
    let result_code = tokio::task::spawn_blocking(move || {
        unsafe {
            // Convert the integer back to a raw pointer inside the new thread
            let instance_ptr = instance_ptr_value as *mut plugin_ffi::UpsclrEngineInstance;
            // This block is unsafe due to FFI call.
            (plugin_arc.upsclr_plugin_preload_upscale)(instance_ptr, scale)
        }
    })
    .await
    .map_err(|e| AppError::InternalServerError(format!("Preload task failed to execute: {}", e)))?;

    match result_code {
        plugin_ffi::UpsclrErrorCode::Success => {
            tracing::info!(
                "Successfully preloaded instance {} for scale {}.",
                uuid,
                params.scale
            );
            Ok(StatusCode::NO_CONTENT)
        }
        err_code => {
            tracing::error!(
                "Preload failed for instance {} scale {}: {:?}",
                uuid,
                params.scale,
                err_code
            );
            Err(AppError::PluginOperationFailed {
                operation: "preload_instance".to_string(),
                details: format!(
                    "Plugin preload failed with code: {:?} for instance {}",
                    err_code, uuid
                ),
            })
        }
    }
}

// --- DELETE /instances/{uuid} ---
// Destroys an active engine instance.
pub async fn delete_instance_handler(
    State((_, instance_manager)): State<(SharedPluginManager, SharedInstanceManager)>,
    Path(uuid): Path<Uuid>, // Extract UUID from the URL path.
) -> Result<StatusCode, AppError> {
    tracing::debug!("Request to delete instance: {}", uuid);
    instance_manager.write()?.delete_instance(&uuid)?;
    tracing::debug!("Successfully deleted instance: {}", uuid);
    Ok(StatusCode::NO_CONTENT)
}

// --- POST /instances/{uuid}/upscale?scale=N ---
// Upscales an image using the specified engine instance.
pub async fn upscale_image(
    State((_, instance_manager)): State<(SharedPluginManager, SharedInstanceManager)>,
    Path(uuid): Path<Uuid>,
    Query(params): Query<ScaleQueryParam>,
    TypedHeader(accept_header): TypedHeader<headers::Accept>,
    request: Request,
) -> Result<Response, AppError> {
    if params.scale <= 1 {
        // Validate scale factor.
        return Err(AppError::BadRequest(
            "Invalid 'scale' parameter. Must be greater than 1.".to_string(),
        ));
    }

    let request_id = Uuid::new_v4();

    tracing::info!(
        "Received upscale request for instance {} with scale {} (req_id={})",
        uuid,
        params.scale,
        request_id
    );

    // Get the content type from the request headers
    let content_type = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string(); // Clone the string to avoid borrowing issues

    // --- Extract image data based on content type ---
    let (file_data, input_content_type) = if content_type.starts_with("multipart/form-data") {
        // Handle multipart/form-data
        extract_multipart_image(request).await?
    } else {
        // Handle direct image upload (non-multipart)
        extract_direct_image(request, &content_type).await?
    };

    // --- Get Instance ---
    // Clone Arc<ActiveInstance> to be moved into the blocking task.
    let instance_arc = instance_manager
        .read()?
        .get_instance(&uuid)
        .ok_or(AppError::InstanceNotFound)?;

    // --- Decode Input Image (handles standard formats and custom x-raw-bitmap) ---
    tracing::debug!("Decoding input image...");
    // Use a scope to ensure file_data can be dropped after decoding
    let (in_data_vec, in_width, in_height, in_channels, in_color_format_plugin) =
        tokio::task::spawn_blocking(move || {
            decode_input_image(&file_data, input_content_type.as_deref())
        })
        .await
        .map_err(|e| {
            AppError::InternalServerError(format!("Image decode task failed to execute: {}", e))
        })??;

    tracing::debug!(
        "Input image decoded: {}x{} {}ch, format: {:?}",
        in_width,
        in_height,
        in_channels,
        in_color_format_plugin
    );

    // --- Prepare Output Buffer ---
    // Calculate dimensions and size for the output image buffer.
    let out_width = in_width.saturating_mul(params.scale as u32);
    let out_height = in_height.saturating_mul(params.scale as u32);
    // Plugin API implies channel count remains the same unless specified otherwise.
    let out_channels = in_channels;
    let out_size = (out_width as usize)
        .saturating_mul(out_height as usize)
        .saturating_mul(out_channels as usize);

    // Sanity check for output buffer size to prevent excessive memory allocation.
    let max_allowed_out_pixels = (20000 * 20000) as usize; // Example: ~1.6GB for 4channel 20k*20k
    let max_allowed_out_bytes = max_allowed_out_pixels.saturating_mul(4); // Max 4 channels
    if out_size == 0 || out_size > max_allowed_out_bytes {
        tracing::error!(
            "Calculated output image size ({} bytes) is too large or zero.",
            out_size
        );
        return Err(AppError::UnprocessableContent(format!(
            "Calculated output image dimensions ({}x{}) result in an excessively large buffer requirement.",
            out_width, out_height
        )));
    }

    // --- Determine Plugin's Output Format based on Client's Accept Header ---
    let preferred_output_format = accept_header
        .0
        .iter()
        .find_map(|mime| OutputFormat::try_from(mime).ok())
        .ok_or_else(|| AppError::NotAcceptable(format!("{:?}", accept_header)))?;

    tracing::debug!(
        "Plugin will be requested to output in format: {:?}",
        preferred_output_format
    );

    // --- Perform Upscaling (Blocking Task) ---
    // Convert the raw pointer into an integer value that can be safely sent between threads
    // This is safe because we're just transferring the address value, not dereferencing it
    let instance_ptr_value = instance_arc.instance_ptr as usize;

    // The plugin will be cloned and sent to the blocking task
    let upscale_task_plugin_arc = instance_arc.plugin.clone();
    let upscale_scale_factor = params.scale;

    tracing::debug!("Spawning blocking task for upscaling operation...");

    // spawn_blocking returns a JoinHandle, which is a future we need to await
    let task_result = tokio::task::spawn_blocking(move || {
        let mut out_data_vec = vec![0; out_size];

        unsafe {
            // Convert the integer back to a raw pointer inside the new thread
            let instance_ptr = instance_ptr_value as *mut plugin_ffi::UpsclrEngineInstance;

            // This block is unsafe due to FFI call.
            let result = (upscale_task_plugin_arc.upsclr_plugin_upscale)(
                instance_ptr,
                upscale_scale_factor,
                in_data_vec.as_ptr(),
                in_data_vec.len(),
                in_width,
                in_height,
                in_channels.into(),             // Convert u8 to u32
                in_color_format_plugin,         // Format of in_data_vec
                out_data_vec.as_mut_ptr(),      // Mutable buffer for output
                out_size,                       // Size of output buffer
                preferred_output_format.into(), // Convert OutputFormat to UpsclrColorFormat
            );

            // Return both the result code and the filled output buffer
            (result, out_data_vec)
        }
    })
    .await
    .map_err(|e| AppError::InternalServerError(format!("Upscale task failed to execute: {}", e)))?;

    // Destructure the result from the task
    let (upscale_result_code, out_data_vec) = task_result;

    if upscale_result_code != plugin_ffi::UpsclrErrorCode::Success {
        tracing::error!(
            "Upscale operation failed with plugin code: {:?}",
            upscale_result_code
        );
        return Err(AppError::PluginOperationFailed {
            operation: "upscale_image".to_string(),
            details: format!(
                "Plugin upscale operation failed with code: {:?} for instance {}",
                upscale_result_code, uuid
            ),
        });
    }
    tracing::debug!("Upscaling operation completed successfully by plugin.");

    // --- Encode Output Image based on Accept Header ---
    // The `out_data_vec` now contains the raw pixel data from the plugin
    // Process the output image in a separate blocking task to avoid holding both buffers in memory
    let result = tokio::task::spawn_blocking(move || {
        encode_output_image(
            &out_data_vec,
            out_width,
            out_height,
            out_channels,
            preferred_output_format,
        )
    })
    .await
    .map_err(|e| {
        AppError::InternalServerError(format!("Image encode task failed to execute: {}", e))
    })?;

    tracing::info!(
        "Upscaling completed successfully for instance {} (req_id={})",
        uuid,
        request_id
    );

    result
}

// --- POST /reset ---
// Query parameters for the reset endpoint
#[derive(serde::Deserialize)]
pub struct ResetQuery {
    // Whether to reset plugins in addition to instances (0/false = no, 1/true = yes)
    #[serde(default, deserialize_with = "deserialize_bool_from_int_optional_query")]
    pub plugins: Option<bool>,
}

// Resets the instances and optionally plugins.
pub async fn reset(
    State((plugin_manager, instance_manager)): State<(SharedPluginManager, SharedInstanceManager)>,
    Query(query_params): Query<ResetQuery>,
) -> Result<StatusCode, AppError> {
    let reset_plugins = query_params.plugins.unwrap_or(false); // Default to false if not provided

    tracing::info!("Received reset request. (plugins={})", reset_plugins);

    let mut plugin_manager = plugin_manager.write()?;
    let mut instance_manager = instance_manager.write()?;

    // First, clear instance manager
    tracing::debug!("Resetting all active engine instances.");
    let instances_state = instance_manager.export_state();
    instance_manager.cleanup(true);
    tracing::debug!("All active engine instances have been reset.");

    // Next, reset plugins if requested
    if reset_plugins {
        tracing::debug!("Resetting all loaded plugins.");
        let plugins_state = plugin_manager.export_state();
        plugin_manager.cleanup(true);
        tracing::debug!("All plugins have been reset.");

        tracing::debug!("Restoring plugins from saved state.");
        unsafe { plugin_manager.import_state(plugins_state) }?;
        tracing::debug!("Plugins have been restored from state.");
    } else {
        tracing::debug!("Skipping plugin reset as per request.");
    }

    // Restore instances from the saved state
    tracing::debug!("Restoring engine instances from saved state.");
    instance_manager.import_state(instances_state, &plugin_manager)?;
    tracing::debug!("Engine instances have been restored from state.");

    tracing::info!(
        "Reset operation completed successfully. (plugins={})",
        reset_plugins
    );

    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Extension,
        body::Body,
        http::{HeaderName, HeaderValue, Method, Request},
        middleware,
        response::Response,
    };
    use http_body_util::BodyExt;
    use tower::{ServiceBuilder, ServiceExt};

    // Helper function to create a test request with specific headers
    fn create_test_request(headers: Vec<(HeaderName, HeaderValue)>) -> Request<Body> {
        let mut request = Request::builder()
            .method(Method::GET)
            .uri("/test")
            .body(Body::empty())
            .unwrap();

        for (name, value) in headers {
            request.headers_mut().insert(name, value);
        }

        request
    }

    // Helper function to create a simple test service
    async fn test_service(
        _request: Request<Body>,
    ) -> Result<Response<Body>, std::convert::Infallible> {
        Ok(Response::builder().status(200).body(Body::empty()).unwrap())
    }

    // Helper function to create a service with validation middleware
    fn create_test_service_with_validation(
        config: SecurityConfig,
    ) -> impl tower::Service<
        Request<Body>,
        Response = Response<Body>,
        Error = std::convert::Infallible,
    > + Clone {
        ServiceBuilder::new()
            .layer(Extension(config))
            .layer(middleware::from_fn(validate_request_security))
            .service_fn(test_service)
    }

    #[tokio::test]
    async fn test_no_validation_configured() {
        // When no validation is configured, all requests should pass
        let config = SecurityConfig {
            require_user_agent_prefix: false,
            require_upsclr_request_header: false,
        };

        let service = create_test_service_with_validation(config);
        let request = create_test_request(vec![]);
        let response = service.oneshot(request).await.unwrap();
        assert_eq!(response.status(), 200);
    }

    #[tokio::test]
    async fn test_user_agent_validation_success_upsclr_slash() {
        let config = SecurityConfig {
            require_user_agent_prefix: true,
            require_upsclr_request_header: false,
        };

        let service = create_test_service_with_validation(config);
        let request = create_test_request(vec![(
            axum::http::header::USER_AGENT,
            HeaderValue::from_static("Upsclr/1.0"),
        )]);

        let response = service.oneshot(request).await.unwrap();
        assert_eq!(response.status(), 200);
    }

    #[tokio::test]
    async fn test_user_agent_validation_success_upsclr_dash() {
        let config = SecurityConfig {
            require_user_agent_prefix: true,
            require_upsclr_request_header: false,
        };

        let service = create_test_service_with_validation(config);
        let request = create_test_request(vec![(
            axum::http::header::USER_AGENT,
            HeaderValue::from_static("Upsclr-client/2.1"),
        )]);

        let response = service.oneshot(request).await.unwrap();
        assert_eq!(response.status(), 200);
    }

    #[tokio::test]
    async fn test_user_agent_validation_failure() {
        let config = SecurityConfig {
            require_user_agent_prefix: true,
            require_upsclr_request_header: false,
        };

        let service = create_test_service_with_validation(config);
        let request = create_test_request(vec![(
            axum::http::header::USER_AGENT,
            HeaderValue::from_static("Mozilla/5.0 (Windows NT 10.0; Win64; x64)"),
        )]);

        let response = service.oneshot(request).await.unwrap();
        assert_eq!(response.status(), 403);

        // Check the response body contains the error message
        let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body_str = String::from_utf8(body_bytes.to_vec()).unwrap();
        assert!(body_str.contains("Invalid User-Agent header"));
        assert!(body_str.contains("expected to start with 'Upsclr/' or 'Upsclr-'"));
        assert!(body_str.contains("Mozilla/5.0"));
    }

    #[tokio::test]
    async fn test_user_agent_validation_missing_header() {
        let config = SecurityConfig {
            require_user_agent_prefix: true,
            require_upsclr_request_header: false,
        };

        let service = create_test_service_with_validation(config);
        let request = create_test_request(vec![]);
        let response = service.oneshot(request).await.unwrap();
        assert_eq!(response.status(), 403);

        let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body_str = String::from_utf8(body_bytes.to_vec()).unwrap();
        assert!(body_str.contains("Missing required User-Agent header"));
    }

    #[tokio::test]
    async fn test_upsclr_request_validation_success() {
        let config = SecurityConfig {
            require_user_agent_prefix: false,
            require_upsclr_request_header: true,
        };

        let service = create_test_service_with_validation(config);
        let request = create_test_request(vec![(
            HeaderName::from_static("upsclr-request"),
            HeaderValue::from_static("true"),
        )]);

        let response = service.oneshot(request).await.unwrap();
        assert_eq!(response.status(), 200);
    }

    #[tokio::test]
    async fn test_upsclr_request_validation_failure_empty() {
        let config = SecurityConfig {
            require_user_agent_prefix: false,
            require_upsclr_request_header: true,
        };

        let service = create_test_service_with_validation(config);
        let request = create_test_request(vec![(
            HeaderName::from_static("upsclr-request"),
            HeaderValue::from_static(""),
        )]);

        let response = service.oneshot(request).await.unwrap();
        assert_eq!(response.status(), 403);

        let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body_str = String::from_utf8(body_bytes.to_vec()).unwrap();
        assert!(body_str.contains("Upsclr-Request header must not be empty"));
    }

    #[tokio::test]
    async fn test_upsclr_request_validation_missing_header() {
        let config = SecurityConfig {
            require_user_agent_prefix: false,
            require_upsclr_request_header: true,
        };

        let service = create_test_service_with_validation(config);
        let request = create_test_request(vec![]);
        let response = service.oneshot(request).await.unwrap();
        assert_eq!(response.status(), 403);

        let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body_str = String::from_utf8(body_bytes.to_vec()).unwrap();
        assert!(body_str.contains("Missing required Upsclr-Request header"));
    }

    #[tokio::test]
    async fn test_both_validations_success() {
        let config = SecurityConfig {
            require_user_agent_prefix: true,
            require_upsclr_request_header: true,
        };

        let service = create_test_service_with_validation(config);
        let request = create_test_request(vec![
            (
                axum::http::header::USER_AGENT,
                HeaderValue::from_static("Upsclr/1.0"),
            ),
            (
                HeaderName::from_static("upsclr-request"),
                HeaderValue::from_static("upscale"),
            ),
        ]);

        let response = service.oneshot(request).await.unwrap();
        assert_eq!(response.status(), 200);
    }

    #[tokio::test]
    async fn test_both_validations_user_agent_fails() {
        let config = SecurityConfig {
            require_user_agent_prefix: true,
            require_upsclr_request_header: true,
        };

        let service = create_test_service_with_validation(config);
        let request = create_test_request(vec![
            (
                axum::http::header::USER_AGENT,
                HeaderValue::from_static("BadClient/1.0"),
            ),
            (
                HeaderName::from_static("upsclr-request"),
                HeaderValue::from_static("upscale"),
            ),
        ]);

        let response = service.oneshot(request).await.unwrap();
        assert_eq!(response.status(), 403);

        let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body_str = String::from_utf8(body_bytes.to_vec()).unwrap();
        assert!(body_str.contains("Invalid User-Agent header"));
    }

    #[tokio::test]
    async fn test_both_validations_upsclr_request_fails() {
        let config = SecurityConfig {
            require_user_agent_prefix: true,
            require_upsclr_request_header: true,
        };

        let service = create_test_service_with_validation(config);
        let request = create_test_request(vec![
            (
                axum::http::header::USER_AGENT,
                HeaderValue::from_static("Upsclr/1.0"),
            ),
            (
                HeaderName::from_static("upsclr-request"),
                HeaderValue::from_static(""),
            ),
        ]);

        let response = service.oneshot(request).await.unwrap();
        assert_eq!(response.status(), 403);

        let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body_str = String::from_utf8(body_bytes.to_vec()).unwrap();
        assert!(body_str.contains("Upsclr-Request header must not be empty"));
    }

    #[tokio::test]
    async fn test_user_agent_validation_case_sensitive() {
        // Test that validation is case-sensitive - "upsclr" should fail
        let config = SecurityConfig {
            require_user_agent_prefix: true,
            require_upsclr_request_header: false,
        };

        let service = create_test_service_with_validation(config);
        let request = create_test_request(vec![(
            axum::http::header::USER_AGENT,
            HeaderValue::from_static("upsclr/1.0"),
        )]);

        let response = service.oneshot(request).await.unwrap();
        assert_eq!(response.status(), 403);

        let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body_str = String::from_utf8(body_bytes.to_vec()).unwrap();
        assert!(body_str.contains("Invalid User-Agent header"));
    }

    #[tokio::test]
    async fn test_invalid_user_agent_header_format() {
        let config = SecurityConfig {
            require_user_agent_prefix: true,
            require_upsclr_request_header: false,
        };

        let service = create_test_service_with_validation(config);
        // Create a request with invalid UTF-8 in the User-Agent header
        let mut request = Request::builder()
            .method(Method::GET)
            .uri("/test")
            .body(Body::empty())
            .unwrap();

        // Insert invalid UTF-8 bytes into the User-Agent header
        request.headers_mut().insert(
            axum::http::header::USER_AGENT,
            HeaderValue::from_bytes(&[0xFF, 0xFE]).unwrap(),
        );

        let response = service.oneshot(request).await.unwrap();
        assert_eq!(response.status(), 403);

        let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body_str = String::from_utf8(body_bytes.to_vec()).unwrap();
        assert!(body_str.contains("Invalid User-Agent header format"));
    }

    #[tokio::test]
    async fn test_invalid_upsclr_request_header_format() {
        let config = SecurityConfig {
            require_user_agent_prefix: false,
            require_upsclr_request_header: true,
        };

        let service = create_test_service_with_validation(config);
        // Create a request with invalid UTF-8 in the Upsclr-Request header
        let mut request = Request::builder()
            .method(Method::GET)
            .uri("/test")
            .body(Body::empty())
            .unwrap();

        // Insert invalid UTF-8 bytes into the Upsclr-Request header
        request.headers_mut().insert(
            HeaderName::from_static("upsclr-request"),
            HeaderValue::from_bytes(&[0xFF, 0xFE]).unwrap(),
        );

        let response = service.oneshot(request).await.unwrap();
        assert_eq!(response.status(), 403);

        let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body_str = String::from_utf8(body_bytes.to_vec()).unwrap();
        assert!(body_str.contains("Invalid Upsclr-Request header format"));
    }

    #[tokio::test]
    async fn test_security_config_clone() {
        // Test that the config can be cloned correctly
        let config = SecurityConfig {
            require_user_agent_prefix: true,
            require_upsclr_request_header: false,
        };

        let config_clone = config.clone();

        assert_eq!(
            config.require_user_agent_prefix,
            config_clone.require_user_agent_prefix
        );
        assert_eq!(
            config.require_upsclr_request_header,
            config_clone.require_upsclr_request_header
        );
    }

    #[tokio::test]
    async fn test_config_debug_format() {
        // Test that the config can be debug formatted (for logging)
        let config = SecurityConfig {
            require_user_agent_prefix: true,
            require_upsclr_request_header: true,
        };

        let debug_str = format!("{:?}", config);
        assert!(debug_str.contains("require_user_agent_prefix: true"));
        assert!(debug_str.contains("require_upsclr_request_header: true"));
    }
}

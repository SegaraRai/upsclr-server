// Contains the Axum handler functions for each API endpoint.
// These handlers process requests, interact with managers, and generate responses.
// Includes handlers that use the combined state tuple (SharedPluginManager, SharedInstanceManager).

use crate::{
    error::{lock_mutex_app_error, AppError},
    headers,
    instance_manager::InstanceManager,
    models::*,
    plugin_ffi,
    plugin_manager::PluginManager,
};
use axum::{
    extract::{Multipart, Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use axum_extra::TypedHeader;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

// --- Shared State Type Aliases (for convenience in handler signatures) ---
#[allow(unused)]
pub type SharedPluginManager = Arc<PluginManager>;
// InstanceManager methods take `&mut self` for `create_instance` and `delete_instance`,
// so it needs a Mutex when shared across Axum's concurrent tasks.
#[allow(unused)]
pub type SharedInstanceManager = Arc<Mutex<InstanceManager>>;

pub const MAX_IMAGE_SIZE_BYTES: usize = 100 * 1024 * 1024; // Example: 100MB limit for input image.

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
    use image::DynamicImage;

    match content_type_str {
       Some("image/jpeg") | Some("image/png") | Some("image/webp") => {
           // Determine image format from content type string.
           let img_format_hint = match content_type_str {
               Some("image/jpeg") => image::ImageFormat::Jpeg,
               Some("image/png") => image::ImageFormat::Png,
               Some("image/webp") => image::ImageFormat::WebP,
               _ => return Err(AppError::ImageProcessingError("Internal error: Mismatched content type in image decoder path.".to_string())), // Should not happen
           };

           let dyn_img: DynamicImage = image::load_from_memory_with_format(file_data, img_format_hint)
               .map_err(|e| AppError::ImageProcessingError(format!("Failed to decode image (format: {:?}): {}", img_format_hint, e)))?;

           let width = dyn_img.width();
           let height = dyn_img.height();
           let channels = if dyn_img.color().has_alpha() {
                4
              } else {
                3
           };

           let data_vec = if channels == 3 {
               dyn_img.to_rgb8().into_raw()
           } else {
               dyn_img.to_rgba8().into_raw()
           };
           Ok((data_vec, width, height, channels, plugin_ffi::UpsclrColorFormat::Rgb))
       }
       Some(ct) if ct.starts_with("image/x-raw-bitmap") => {
           // Parse the 16-byte header for custom "image/x-raw-bitmap" format.
           // Header: ["R"]["B"](2B), ColorFormat(1B), Channels(1B), DataSize(4B LE), Width(4B LE), Height(4B LE)
           if file_data.len() < 16 {
               return Err(AppError::ImageProcessingError(
                   "x-raw-bitmap data is too short for its 16-byte header.".to_string(),
               ));
           }

           if &file_data[0..2] != b"RB" { // Magic bytes check.
               return Err(AppError::ImageProcessingError(
                   "Invalid magic bytes for x-raw-bitmap. Expected 'RB'.".to_string(),
               ));
           }

           let header_color_format_byte = file_data[2]; // 0x01 for RGB/RGBA, 0x02 for BGR/BGRA
           let header_channels_byte = file_data[3];     // 3 for RGB/BGR, 4 for RGBA/BGRA
           let data_size_from_header = LittleEndian::read_u32(&file_data[4..8]); // Size of pixel data *after* header
           let width = LittleEndian::read_u32(&file_data[8..12]);
           let height = LittleEndian::read_u32(&file_data[12..16]);

           if width == 0 || height == 0 {
               return Err(AppError::ImageProcessingError("x-raw-bitmap header indicates zero width or height.".to_string()));
           }
           if header_channels_byte != 3 && header_channels_byte != 4 {
                return Err(AppError::ImageProcessingError(format!(
                   "x-raw-bitmap header specifies an unsupported channel count: {}. Expected 3 or 4.", header_channels_byte
               )));
           }

           // Map header's ColorFormat byte to the plugin's plugin_ffi::UpsclrColorFormat enum.
           let plugin_color_format = match header_color_format_byte {
               0x01 => plugin_ffi::UpsclrColorFormat::Rgb,  // Covers RGB and RGBA (plugin gets channels separately)
               0x02 => plugin_ffi::UpsclrColorFormat::Bgr,  // Covers BGR and BGRA
               invalid_byte => return Err(AppError::ImageProcessingError(format!(
                   "Invalid ColorFormat byte (0x{:02X}) in x-raw-bitmap header. Expected 0x01 or 0x02.", invalid_byte
               ))),
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
                   file_data.len(), expected_total_size
               )));
           }

           // Extract pixel data (skip the 16-byte header).
           let data_vec = file_data[16..].to_vec();

           Ok((data_vec, width, height, header_channels_byte, plugin_color_format))
       }
       None => {
           // Try to auto-detect format if no content type was specified.
           let dyn_img: DynamicImage = image::load_from_memory(file_data)
               .map_err(|e| AppError::ImageProcessingError(format!("Failed to auto-detect and decode image: {}", e)))?;

           let width = dyn_img.width();
           let height = dyn_img.height();
           let channels = if dyn_img.color().has_alpha() { 4 } else { 3 };
           let data_vec = if channels == 3 {
               dyn_img.to_rgb8().into_raw()
           } else {
               dyn_img.to_rgba8().into_raw()
           };

           Ok((data_vec, width, height, channels, plugin_ffi::UpsclrColorFormat::Rgb))
       }
       Some(unknown_content_type) => {
           Err(AppError::UnsupportedMediaType(format!(
               "Content type '{}' is not supported. Expected 'image/jpeg', 'image/png', 'image/webp', or 'image/x-raw-bitmap'.",
               unknown_content_type
           )))
       }
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
                    )))
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

// --- GET /plugins ---
// Lists all loaded plugins and their available engines.
pub async fn get_plugins(
    State(plugin_manager): State<SharedPluginManager>,
) -> Result<Json<Vec<PluginDescriptionResponse>>, AppError> {
    let descriptions: Vec<PluginDescriptionResponse> = plugin_manager
        .plugins
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
    State((_, instance_manager_mutex)): State<(SharedPluginManager, SharedInstanceManager)>,
) -> Result<Json<Vec<InstanceInfoForList>>, AppError> {
    let manager_locked = lock_mutex_app_error(&instance_manager_mutex, "list_instances")?;
    let instances_info = manager_locked.list_instances_info();
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
    State((plugin_manager, instance_manager_mutex)): State<(
        SharedPluginManager,
        SharedInstanceManager,
    )>,
    Query(query_params): Query<CreateInstanceQuery>, // Use CreateInstanceQuery from models
    Json(payload): Json<CreateInstanceRequest>,
) -> Result<Json<CreateInstanceResponse>, AppError> {
    let dry_run_active = query_params.dry_run.unwrap_or(false); // Default to false
    tracing::info!(
       "Attempting to create/validate instance for plugin_id: '{}', engine_name: '{}', dry_run: {}",
       payload.plugin_id, payload.engine_name, dry_run_active
    );

    // Find the specified plugin by its ID.
    let plugin_arc = plugin_manager
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
            warning_count: c_result.warning_count,
            warning_messages: warnings,
            error_messages: errors,
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
            warning_count: 0,
            warning_messages: vec![],
            error_messages: vec![],
        })
    };

    // If dry_run is active, return validation result without creating an instance.
    if dry_run_active {
        tracing::info!("Dry run completed for instance creation.");
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
    // Lock the InstanceManager to modify its state.
    let mut manager_locked = lock_mutex_app_error(&instance_manager_mutex, "create_instance")?;
    let active_instance_arc =
        manager_locked.create_instance(plugin_arc.clone(), engine_index, &config_json_str)?; // Pass Rust string

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
    State((_, instance_manager_mutex)): State<(SharedPluginManager, SharedInstanceManager)>,
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
    let instance_arc = {
        // Scope for MutexGuard
        let manager_locked = lock_mutex_app_error(&instance_manager_mutex, "preload_get_instance")?;
        manager_locked
            .get_instance(&uuid)
            .ok_or(AppError::InstanceNotFound)?
    };

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
            Ok(StatusCode::OK)
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
    State((_, instance_manager_mutex)): State<(SharedPluginManager, SharedInstanceManager)>,
    Path(uuid): Path<Uuid>, // Extract UUID from the URL path.
) -> Result<StatusCode, AppError> {
    tracing::info!("Request to delete instance: {}", uuid);
    let mut manager_locked = lock_mutex_app_error(&instance_manager_mutex, "delete_instance")?;
    manager_locked.delete_instance(&uuid)?; // This now returns AppError::InstanceNotFound on failure.
    tracing::info!("Successfully deleted instance: {}", uuid);
    Ok(StatusCode::NO_CONTENT) // HTTP 204 No Content on successful deletion.
}

// --- POST /instances/{uuid}/upscale?scale=N ---
// Upscales an image using the specified engine instance.
pub async fn upscale_image(
    State((_, instance_manager_mutex)): State<(SharedPluginManager, SharedInstanceManager)>,
    Path(uuid): Path<Uuid>,
    Query(params): Query<ScaleQueryParam>,
    TypedHeader(accept_header): TypedHeader<headers::Accept>,
    mut multipart: Multipart,
) -> Result<Response, AppError> {
    if params.scale <= 1 {
        // Validate scale factor.
        return Err(AppError::BadRequest(
            "Invalid 'scale' parameter. Must be greater than 1.".to_string(),
        ));
    }

    tracing::info!(
        "Upscaling request for instance {}, scale {}",
        uuid,
        params.scale
    );

    // --- Parse Multipart for "file" field ---
    // Look for the "file" field, ignoring any other fields
    let (file_data, input_content_type) = {
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
            tracing::info!(
                "Ignored {} non-file fields in multipart request",
                ignored_fields
            );
        }

        match file_data_opt {
            Some(data) => (data, content_type_opt),
            None => {
                return Err(AppError::BadRequest(
                    "Missing 'file' field in multipart request.".to_string(),
                ))
            }
        }
    };
    // Process any remaining fields (ignoring them) to drain the multipart stream
    let mut extra_field_count = 0;
    while let Some(field) = multipart.next_field().await? {
        extra_field_count += 1;
        let field_name = field.name().unwrap_or("unnamed").to_string();
        tracing::warn!("Ignoring extra multipart field: {}", field_name);
    }
    if extra_field_count > 0 {
        tracing::debug!(
            "Ignored {} additional multipart fields after processing 'file'",
            extra_field_count
        );
    }

    // --- Get Instance ---
    // Clone Arc<ActiveInstance> to be moved into the blocking task.
    let instance_arc = {
        // Scope for MutexGuard
        let manager_locked = lock_mutex_app_error(&instance_manager_mutex, "upscale_get_instance")?;
        manager_locked
            .get_instance(&uuid)
            .ok_or(AppError::InstanceNotFound)?
    };

    // --- Decode Input Image (handles standard formats and custom x-raw-bitmap) ---
    tracing::debug!("Decoding input image...");
    let (in_data_vec, in_width, in_height, in_channels, in_color_format_plugin) =
        decode_input_image(&file_data, input_content_type.as_deref())?; // Use our decode_input_image function
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
           "Calculated output image dimensions ({}x{}) result in an excessively large buffer requirement.", out_width, out_height
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

    // Create separate vectors for use in the task
    let in_data_vec_for_task = in_data_vec.clone();
    let mut out_data_vec_for_task = vec![0; out_size]; // Create a fresh buffer for the task

    tracing::debug!("Spawning blocking task for upscaling operation...");
    // spawn_blocking returns a JoinHandle, which is a future we need to await
    let task_result = tokio::task::spawn_blocking(move || {
        unsafe {
            // Convert the integer back to a raw pointer inside the new thread
            let instance_ptr = instance_ptr_value as *mut plugin_ffi::UpsclrEngineInstance;

            // This block is unsafe due to FFI call.
            let result = (upscale_task_plugin_arc.upsclr_plugin_upscale)(
                instance_ptr,
                upscale_scale_factor,
                in_data_vec_for_task.as_ptr(), // Input image data
                in_data_vec_for_task.len(),    // Size of input data
                in_width,
                in_height,
                in_channels.into(),                 // Convert u8 to u32
                in_color_format_plugin,             // Format of in_data_vec
                out_data_vec_for_task.as_mut_ptr(), // Mutable buffer for output
                out_size,                           // Size of output buffer
                preferred_output_format.into(),     // Convert OutputFormat to UpsclrColorFormat
            );

            // Return both the result code and the filled output buffer
            (result, out_data_vec_for_task)
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
    // in `preferred_out_plugin_format`.
    encode_output_image(
        out_data_vec.as_slice(),
        out_width,
        out_height,
        out_channels,
        preferred_output_format,
    )
}

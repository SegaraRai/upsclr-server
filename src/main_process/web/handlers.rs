// API handlers for the web server

use super::{
    SharedPluginManager,
    error::ApiError,
    extract_request_data::extract_request_image,
    headers,
    image_codec::{OutputFormat, decode_input_image, encode_output_image},
    models::*,
};
use crate::models::{EngineInstanceConfig, PreloadParams, UpscaleParams};
use axum::{
    Json,
    extract::{Path, Query, Request, State},
    http::StatusCode,
    response::Response,
};
use axum_extra::TypedHeader;
use tracing::{debug, info};
use uuid::Uuid;

// --- GET /plugins ---
// Lists all loaded plugins and their available engines
pub async fn get_plugins(
    State(plugin_manager): State<SharedPluginManager>,
) -> Result<Json<Vec<ApiPluginInfo>>, ApiError> {
    let plugin_infos = plugin_manager.read().await.get_all_plugins().await;

    let descriptions = plugin_infos
        .into_iter()
        .map(ApiPluginInfo::try_from)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| {
            ApiError::InternalServerError(format!("Failed to convert plugin info: {}", err))
        })?;

    Ok(Json(descriptions))
}

// --- GET /instances ---
// Lists all active engine instances
pub async fn list_instances(
    State(plugin_manager): State<SharedPluginManager>,
) -> Result<Json<Vec<ApiEngineInstance>>, ApiError> {
    let plugin_infos = plugin_manager.read().await.get_all_plugins().await;

    let instances = plugin_infos
        .into_iter()
        .flat_map(|plugin_info| {
            plugin_info
                .engine_instances
                .into_iter()
                .map(ApiEngineInstance::try_from)
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| {
            ApiError::InternalServerError(format!("Failed to convert engine instances: {}", err))
        })?;

    Ok(Json(instances))
}

// --- POST /instances ---
// Creates a new engine instance or validates configuration (if dry_run=true)
pub async fn create_instance(
    State(plugin_manager): State<SharedPluginManager>,
    Query(query_params): Query<CreateInstanceQuery>,
    Json(payload): Json<CreateInstanceRequest>,
) -> Result<Json<CreateInstanceResponse>, ApiError> {
    debug!(
        "Create instance request: plugin_id={}, engine_name={}, dry_run={}",
        payload.plugin_id,
        payload.engine_name,
        query_params.dry_run.unwrap_or(false)
    );

    let plugin_manager_guard = plugin_manager.read().await;

    // Validate the engine configuration
    let validation_result = plugin_manager_guard
        .validate_engine_config(
            payload.plugin_id,
            payload.engine_name.clone(),
            payload.config.clone(),
        )
        .await?;

    let validation_desc = ValidationResultDesc {
        is_valid: validation_result.is_valid,
        error_count: validation_result.error_messages.len(),
        error_messages: validation_result.error_messages,
        warning_count: validation_result.warning_messages.len(),
        warning_messages: validation_result.warning_messages,
    };

    // If this is a dry run, just return the validation results
    if query_params.dry_run.unwrap_or(false) {
        return Ok(Json(CreateInstanceResponse {
            instance_id: None,
            validation: Some(validation_desc),
        }));
    }

    // If the configuration is not valid, return an error
    if !validation_desc.is_valid {
        return Err(ApiError::BadRequest(format!(
            "Engine configuration is invalid: {:?}",
            validation_desc.error_messages
        )));
    }

    // Create the engine instance
    let instance = plugin_manager_guard
        .create_engine_instance(EngineInstanceConfig {
            instance_id: Uuid::new_v4(),
            plugin_id: payload.plugin_id,
            engine_name: payload.engine_name,
            engine_config: serde_json::to_string(&payload.config).unwrap(),
        })
        .await?;

    Ok(Json(CreateInstanceResponse {
        instance_id: Some(instance.config.instance_id),
        validation: Some(validation_desc),
    }))
}

// --- DELETE /instances/{uuid} ---
// Deletes an engine instance
pub async fn delete_instance(
    State(plugin_manager): State<SharedPluginManager>,
    Path(instance_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    debug!("Delete instance request: instance_id={}", instance_id);

    let plugin_manager_guard = plugin_manager.read().await;

    // Find the instance to get plugin_id
    // First we need to get all loaded plugins
    let plugins = plugin_manager_guard.get_all_plugins().await;

    // Find the plugin that contains this instance
    let mut found_plugin_id = None;
    for plugin in plugins {
        for instance in &plugin.engine_instances {
            if instance.config.instance_id == instance_id {
                found_plugin_id = Some(plugin.plugin_id);
                break;
            }
        }
        if found_plugin_id.is_some() {
            break;
        }
    }

    let plugin_id = found_plugin_id
        .ok_or_else(|| ApiError::NotFound(format!("Instance with ID {} not found", instance_id)))?;

    // Destroy the engine instance
    plugin_manager_guard
        .destroy_engine_instance(plugin_id, instance_id)
        .await
        .map_err(|e| {
            ApiError::InternalServerError(format!("Failed to delete instance: {:?}", e))
        })?;

    Ok(StatusCode::NO_CONTENT)
}

// --- POST /instances/{uuid}/preload?scale=N ---
// Preloads an engine instance for a specific scale factor
pub async fn preload_instance(
    State(plugin_manager): State<SharedPluginManager>,
    Path(instance_id): Path<Uuid>,
    Query(params): Query<ScaleQueryParam>,
) -> Result<StatusCode, ApiError> {
    if params.scale <= 1 {
        return Err(ApiError::BadRequest(
            "Invalid scale parameter: must be greater than 1".into(),
        ));
    }

    debug!(
        "Preload instance request: instance_id={}, scale={}",
        instance_id, params.scale
    );

    let plugin_manager_guard = plugin_manager.read().await;

    // Find the instance to get plugin_id
    // First we need to get all loaded plugins
    let plugins = plugin_manager_guard.get_all_plugins().await;

    // Find the plugin that contains this instance
    let mut found_plugin_id = None;
    for plugin in plugins {
        for instance in &plugin.engine_instances {
            if instance.config.instance_id == instance_id {
                found_plugin_id = Some(plugin.plugin_id);
                break;
            }
        }
        if found_plugin_id.is_some() {
            break;
        }
    }

    let plugin_id = found_plugin_id
        .ok_or_else(|| ApiError::NotFound(format!("Instance with ID {} not found", instance_id)))?;

    // Prepare preload parameters
    // Since we don't have actual dimensions yet, we'll use a reasonable size
    // that scales with the requested scale factor
    let max_dim = 1920; // Base dimension for a typical HD image
    let preload_params = PreloadParams {
        width: max_dim * params.scale,
        height: max_dim * params.scale,
        channels: 3, // Typical RGB
    };

    // Preload the instance
    plugin_manager_guard
        .preload(plugin_id, instance_id, preload_params)
        .await
        .map_err(|e| {
            ApiError::InternalServerError(format!("Failed to preload instance: {:?}", e))
        })?;

    Ok(StatusCode::NO_CONTENT)
}

// --- POST /instances/{uuid}/upscale?scale=N ---
// Upscales an image using the specified engine instance
pub async fn upscale_image(
    State(plugin_manager): State<SharedPluginManager>,
    Path(instance_id): Path<Uuid>,
    Query(params): Query<ScaleQueryParam>,
    TypedHeader(accept_header): TypedHeader<headers::Accept>,
    request: Request,
) -> Result<Response, ApiError> {
    if params.scale <= 1 {
        return Err(ApiError::BadRequest(
            "Invalid scale parameter: must be greater than 1".into(),
        ));
    }

    let request_id = Uuid::new_v4();
    info!(
        "Upscale request: instance_id={}, scale={}, request_id={}",
        instance_id, params.scale, request_id
    );

    // Extract image data based on content type
    let (file_data, input_content_type) = extract_request_image(request).await?;

    // Decode the input image
    let (in_data_vec, in_width, in_height, in_channels, in_color_format) =
        tokio::task::spawn_blocking(move || {
            decode_input_image(&file_data, input_content_type.as_deref())
        })
        .await
        .map_err(|e| ApiError::InternalServerError(format!("Image decode task failed: {}", e)))??;

    debug!(
        "Input image decoded: {}x{} {}ch, format: {:?}",
        in_width, in_height, in_channels, in_color_format
    );

    // Determine preferred output format from Accept header
    let preferred_output_format = accept_header
        .0
        .iter()
        .find_map(|mime| OutputFormat::try_from(mime).ok())
        .ok_or_else(|| ApiError::NotAcceptable("No acceptable output format found".to_string()))?;

    // Find the plugin_id for the instance
    let plugin_manager_guard = plugin_manager.read().await;

    // Get all loaded plugins
    let plugins = plugin_manager_guard.get_all_plugins().await;

    // Find the plugin that contains this instance
    let mut found_plugin_id = None;
    for plugin in plugins {
        for instance in &plugin.engine_instances {
            if instance.config.instance_id == instance_id {
                found_plugin_id = Some(plugin.plugin_id);
                break;
            }
        }
        if found_plugin_id.is_some() {
            break;
        }
    }

    let plugin_id = found_plugin_id
        .ok_or_else(|| ApiError::NotFound(format!("Instance with ID {} not found", instance_id)))?;

    // Prepare upscale parameters
    let upscale_params = UpscaleParams {
        scale: params.scale,
        input_width: in_width,
        input_height: in_height,
        channels: in_channels,
        input_color_format: in_color_format,
        desired_color_format: crate::models::ColorFormat::from(preferred_output_format),
        input_data: in_data_vec,
    };

    // Perform upscaling
    let result = plugin_manager_guard
        .upscale(plugin_id, instance_id, upscale_params)
        .await
        .map_err(|e| ApiError::InternalServerError(format!("Upscale operation failed: {:?}", e)))?;

    debug!(
        "Upscale completed: {}x{} to {}x{} in {} μs",
        in_width, in_height, result.output_width, result.output_height, result.processing_time_us
    );

    // Encode the output image
    encode_output_image(
        &result.output_data,
        result.output_width,
        result.output_height,
        result.channels,
        preferred_output_format,
    )
}

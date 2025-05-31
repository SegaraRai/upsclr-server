// Defines data structures for API request and response bodies,
// using Serde for JSON serialization and deserialization.

use crate::plugin_manager::{EngineInfo, PluginInfo}; // Use the Rust-native structs from plugin_manager
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// Describes a plugin and its available engines for the GET /plugins endpoint.
#[derive(Serialize, Debug)]
pub struct PluginDescriptionResponse {
    // Embeds fields from PluginInfo (name, version, description) directly into this struct's JSON.
    #[serde(flatten)]
    pub plugin_info: PluginInfo,
    // Lists all engines provided by this plugin.
    pub engines: Vec<EngineInfo>,
}

// Request body structure for POST /instances to create a new engine instance.
#[derive(Deserialize, Debug)]
pub struct CreateInstanceRequest {
    // Unique identifier for the plugin.
    pub plugin_id: String,
    // Name of the specific engine within the plugin to instantiate.
    pub engine_name: String,
    // Engine-specific configuration, passed as a flexible JSON value.
    // The plugin itself (via its JSON schema) defines the structure of this config.
    pub config: serde_json::Value,
}

// Response structure for POST /instances.
#[derive(Serialize, Debug)]
pub struct CreateInstanceResponse {
    // The unique UUID assigned to the newly created (or validated) engine instance.
    // Will be None if dry_run was true and no instance was actually created.
    pub instance_id: Option<Uuid>,
    // Provides the result of the configuration validation.
    // This is always present:
    // - For dry_run=true, it's the primary result.
    // - For dry_run=false, it contains any warnings or confirms validity before creation.
    pub validation: Option<ValidationResultDesc>, // Kept as Option in case validation is skipped or yields no info
}

// Describes the outcome of an engine configuration validation attempt.
// This structure is derived from the plugin's FFI validation result.
#[derive(Serialize, Deserialize, Debug, Clone)] // Added Clone for easier handling
pub struct ValidationResultDesc {
    pub is_valid: bool,
    pub error_count: usize,
    pub error_messages: Vec<String>,
    pub warning_count: usize,
    pub warning_messages: Vec<String>,
}

// Query parameters for endpoints that accept a 'scale' factor,
// such as /preload and /upscale.
#[derive(Deserialize, Debug)]
pub struct ScaleQueryParam {
    // The desired upscale factor (e.g., 2, 3, or 4).
    pub scale: i32,
}

// Query parameters specific to the POST /instances endpoint.
#[derive(Deserialize, Debug)]
pub struct CreateInstanceQuery {
    // If Some(true), the server should only validate the configuration
    // without creating an actual engine instance.
    // If Some(false) or None, an instance is created if validation passes.
    // Defaults to false if not provided.
    #[serde(default, deserialize_with = "deserialize_bool_from_int_optional_query")]
    pub dry_run: Option<bool>,
}

// Custom deserializer for dry_run query parameter (0, 1, true, false, or missing).
pub fn deserialize_bool_from_int_optional_query<'de, D>(
    deserializer: D,
) -> Result<Option<bool>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    // This handles cases where dry_run might be sent as a string "0" or "1" from query params.
    // Axum's Query extractor is pretty good, but explicit handling can be robust.
    // For simplicity with Axum's Query, often just Option<bool> or Option<u8> and manual check is fine too.
    // Here, assuming it comes as a number or can be parsed into one.
    let s: Option<String> = Option::deserialize(deserializer)?;
    match s.as_deref() {
        Some("1") => Ok(Some(true)),
        Some("true") => Ok(Some(true)),
        Some("0") => Ok(Some(false)),
        Some("false") => Ok(Some(false)),
        None => Ok(None), // Parameter not present
        Some(other) => Err(serde::de::Error::invalid_value(
            serde::de::Unexpected::Str(other),
            &"0, 1, true, false, or not present",
        )),
    }
}

// Simplified information for listing instances via GET /instances.
#[derive(Serialize, Debug)]
pub struct InstanceInfoForList {
    pub instance_id: Uuid,
    pub plugin_id: String,
    pub plugin_name: String,
    pub engine_name: String,
    // Potentially include more details like creation time or basic config summary if needed.
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;

    #[test]
    fn test_create_instance_request_deserialization() {
        let json = r#"{
            "plugin_id": "test_plugin",
            "engine_name": "test_engine",
            "config": {"param1": "value1", "param2": 42}
        }"#;

        let request: CreateInstanceRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.plugin_id, "test_plugin");
        assert_eq!(request.engine_name, "test_engine");
        assert_eq!(request.config["param1"], "value1");
        assert_eq!(request.config["param2"], 42);
    }

    #[test]
    fn test_create_instance_response_serialization() {
        let uuid = Uuid::new_v4();
        let validation = ValidationResultDesc {
            is_valid: true,
            error_count: 0,
            error_messages: vec![],
            warning_count: 1,
            warning_messages: vec!["Test warning".to_string()],
        };

        let response = CreateInstanceResponse {
            instance_id: Some(uuid),
            validation: Some(validation),
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains(&uuid.to_string()));
        assert!(json.contains("Test warning"));
        assert!(json.contains("\"is_valid\":true"));
    }

    #[test]
    fn test_validation_result_desc_serialization() {
        let validation = ValidationResultDesc {
            is_valid: false,
            error_count: 2,
            error_messages: vec!["Error 1".to_string(), "Error 2".to_string()],
            warning_count: 0,
            warning_messages: vec![],
        };

        let json = serde_json::to_string(&validation).unwrap();
        let parsed: ValidationResultDesc = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.is_valid, false);
        assert_eq!(parsed.error_count, 2);
        assert_eq!(parsed.error_messages.len(), 2);
        assert_eq!(parsed.error_messages[0], "Error 1");
        assert_eq!(parsed.warning_count, 0);
    }

    #[test]
    fn test_scale_query_param_deserialization() {
        let json = r#"{"scale": 4}"#;
        let param: ScaleQueryParam = serde_json::from_str(json).unwrap();
        assert_eq!(param.scale, 4);
    }

    #[test]
    fn test_deserialize_bool_from_int_optional_query() {
        // Test true values
        assert_eq!(
            deserialize_bool_from_int_optional_query(&mut serde_json::Deserializer::from_str(
                "\"1\""
            ))
            .unwrap(),
            Some(true)
        );
        assert_eq!(
            deserialize_bool_from_int_optional_query(&mut serde_json::Deserializer::from_str(
                "\"true\""
            ))
            .unwrap(),
            Some(true)
        );

        // Test false values
        assert_eq!(
            deserialize_bool_from_int_optional_query(&mut serde_json::Deserializer::from_str(
                "\"0\""
            ))
            .unwrap(),
            Some(false)
        );
        assert_eq!(
            deserialize_bool_from_int_optional_query(&mut serde_json::Deserializer::from_str(
                "\"false\""
            ))
            .unwrap(),
            Some(false)
        );

        // Test null
        assert_eq!(
            deserialize_bool_from_int_optional_query(&mut serde_json::Deserializer::from_str(
                "null"
            ))
            .unwrap(),
            None
        );
    }

    #[test]
    fn test_create_instance_query_dry_run_parsing() {
        // Test dry_run=true
        let json = r#"{"dry_run": "1"}"#;
        let query: CreateInstanceQuery = serde_json::from_str(json).unwrap();
        assert_eq!(query.dry_run, Some(true));

        // Test dry_run=false
        let json = r#"{"dry_run": "0"}"#;
        let query: CreateInstanceQuery = serde_json::from_str(json).unwrap();
        assert_eq!(query.dry_run, Some(false));

        // Test no dry_run
        let json = r#"{}"#;
        let query: CreateInstanceQuery = serde_json::from_str(json).unwrap();
        assert_eq!(query.dry_run, None);
    }

    #[test]
    fn test_instance_info_for_list_serialization() {
        let uuid = Uuid::new_v4();
        let info = InstanceInfoForList {
            instance_id: uuid,
            plugin_id: "test_plugin".to_string(),
            plugin_name: "Test Plugin".to_string(),
            engine_name: "Test Engine".to_string(),
        };

        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains(&uuid.to_string()));
        assert!(json.contains("test_plugin"));
        assert!(json.contains("Test Plugin"));
        assert!(json.contains("Test Engine"));
    }
}

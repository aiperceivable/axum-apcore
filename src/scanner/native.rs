// Native Axum scanner — discover routes via a metadata registry.
//
// Unlike FastAPI, Axum's Router is type-erased and does not expose route
// metadata at runtime. This scanner uses a compile-time metadata registry
// populated via the `ap_handler!` macro or manual `RouteMetadata` registration.
//
// For OpenAPI-based scanning (e.g. a utoipa-generated OpenAPI 3.x document),
// use the "openapi" feature and `OpenAPIScanner` instead.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use apcore_toolkit::{
    deduplicate_ids, filter_modules, generate_suggested_alias, infer_annotations_from_method,
    ScannedModule,
};
use async_trait::async_trait;

use crate::errors::AxumApcoreError;
use crate::scanner::AxumScanner;

/// Metadata for a single Axum route, used for scanning.
#[derive(Debug, Clone)]
pub struct RouteMetadata {
    /// HTTP method (GET, POST, PUT, DELETE, PATCH).
    pub method: String,
    /// URL path (e.g., "/api/users/:id").
    pub path: String,
    /// Handler function name or target string.
    pub handler_name: String,
    /// Human-readable description.
    pub description: String,
    /// Tags for grouping.
    pub tags: Vec<String>,
    /// JSON Schema for inputs.
    pub input_schema: serde_json::Value,
    /// JSON Schema for outputs.
    pub output_schema: serde_json::Value,
    /// Optional full documentation.
    pub documentation: Option<String>,
}

/// Global registry for route metadata.
///
/// Handlers register their metadata here (via `register_route` or the
/// `ap_handler!` macro), and the NativeAxumScanner reads it during scanning.
static ROUTE_REGISTRY: std::sync::LazyLock<Arc<Mutex<Vec<RouteMetadata>>>> =
    std::sync::LazyLock::new(|| Arc::new(Mutex::new(Vec::new())));

/// Register route metadata for scanning.
pub fn register_route(metadata: RouteMetadata) {
    let mut registry = ROUTE_REGISTRY.lock().expect("route registry lock poisoned");
    registry.push(metadata);
}

/// Clear all registered route metadata (for testing).
pub fn clear_routes() {
    let mut registry = ROUTE_REGISTRY.lock().expect("route registry lock poisoned");
    registry.clear();
}

/// Get a snapshot of all registered route metadata.
pub fn get_registered_routes() -> Vec<RouteMetadata> {
    let registry = ROUTE_REGISTRY.lock().expect("route registry lock poisoned");
    registry.clone()
}

/// Native scanner that reads from the compile-time route metadata registry.
pub struct NativeAxumScanner;

impl NativeAxumScanner {
    pub fn new() -> Self {
        Self
    }

    /// Scan from an explicit list of routes (bypasses the global registry).
    pub fn scan_routes(
        &self,
        routes: &[RouteMetadata],
        include: Option<&str>,
        exclude: Option<&str>,
    ) -> Result<Vec<ScannedModule>, AxumApcoreError> {
        let modules: Vec<ScannedModule> = routes
            .iter()
            .map(|meta| self.metadata_to_module(meta))
            .collect();

        let modules = deduplicate_ids(modules);
        let modules = filter_modules(&modules, include, exclude)?;
        Ok(modules)
    }

    /// Convert a RouteMetadata into a ScannedModule.
    fn metadata_to_module(&self, meta: &RouteMetadata) -> ScannedModule {
        let tag = if meta.tags.is_empty() {
            extract_tag_from_path(&meta.path)
        } else {
            meta.tags[0].clone()
        };

        let module_id = format!(
            "{}.{}.{}",
            tag,
            meta.handler_name,
            meta.method.to_lowercase()
        );

        let annotations = infer_annotations_from_method(&meta.method);

        let mut metadata_map = HashMap::new();
        metadata_map.insert(
            "http_method".into(),
            serde_json::Value::String(meta.method.clone()),
        );
        metadata_map.insert(
            "url_path".into(),
            serde_json::Value::String(meta.path.clone()),
        );

        // A CLI/MCP-friendly alias derived from the HTTP route, e.g.
        // `GET /api/users/:id` -> "users-get". Surface adapters prefer this
        // over the dotted module_id when present (apcore-toolkit 0.5+).
        let suggested_alias = generate_suggested_alias(&meta.path, &meta.method);

        // `ScannedModule::new` + field assignment rather than a struct
        // literal: the struct is not `#[non_exhaustive]` and the toolkit
        // bound is open-ended, so a literal would break on any added field.
        let mut module = ScannedModule::new(
            module_id,
            meta.description.clone(),
            meta.input_schema.clone(),
            meta.output_schema.clone(),
            meta.tags.clone(),
            format!("axum::{}", meta.handler_name),
        );
        module.annotations = Some(annotations);
        module.documentation = meta.documentation.clone();
        module.suggested_alias = Some(suggested_alias);
        module.metadata = metadata_map;
        module
    }
}

impl Default for NativeAxumScanner {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AxumScanner for NativeAxumScanner {
    async fn scan(
        &self,
        _app: &axum::Router,
        include: Option<&str>,
        exclude: Option<&str>,
    ) -> Result<Vec<ScannedModule>, AxumApcoreError> {
        let routes = get_registered_routes();

        let modules: Vec<ScannedModule> = routes
            .iter()
            .map(|meta| self.metadata_to_module(meta))
            .collect();

        let modules = deduplicate_ids(modules);
        let modules = filter_modules(&modules, include, exclude)?;

        Ok(modules)
    }

    fn source_name(&self) -> &str {
        "native"
    }
}

/// Extract a tag from a URL path.
///
/// Takes the first meaningful path segment: "/api/v1/users/:id" -> "users".
fn extract_tag_from_path(path: &str) -> String {
    path.split('/')
        .find(|s| !s.is_empty() && !s.starts_with(':') && *s != "api" && !s.starts_with('v'))
        .unwrap_or("default")
        .to_string()
}

/// Convenience macro for registering an Axum handler with apcore metadata.
///
/// # Example
///
/// ```ignore
/// use axum_apcore::ap_handler;
/// use serde_json::json;
///
/// ap_handler! {
///     method: "GET",
///     path: "/api/users/:id",
///     handler: get_user,
///     description: "Get a user by ID",
///     tags: ["users"],
///     input_schema: json!({"type": "object", "properties": {"id": {"type": "string"}}}),
///     output_schema: json!({"type": "object", "properties": {"name": {"type": "string"}}}),
/// }
/// ```
#[macro_export]
macro_rules! ap_handler {
    (
        method: $method:expr,
        path: $path:expr,
        handler: $handler:ident,
        description: $desc:expr,
        tags: [$($tag:expr),* $(,)?],
        input_schema: $input:expr,
        output_schema: $output:expr $(,)?
    ) => {
        $crate::scanner::native::register_route($crate::scanner::native::RouteMetadata {
            method: $method.to_string(),
            path: $path.to_string(),
            handler_name: stringify!($handler).to_string(),
            description: $desc.to_string(),
            tags: vec![$($tag.to_string()),*],
            input_schema: $input,
            output_schema: $output,
            documentation: None,
        });
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_route() -> RouteMetadata {
        RouteMetadata {
            method: "GET".into(),
            path: "/api/users/:id".into(),
            handler_name: "get_user".into(),
            description: "Get a user by ID".into(),
            tags: vec!["users".into()],
            input_schema: json!({"type": "object", "properties": {"id": {"type": "string"}}}),
            output_schema: json!({"type": "object", "properties": {"name": {"type": "string"}}}),
            documentation: None,
        }
    }

    #[test]
    fn test_metadata_to_module() {
        let scanner = NativeAxumScanner::new();
        let meta = sample_route();
        let module = scanner.metadata_to_module(&meta);

        assert_eq!(module.module_id, "users.get_user.get");
        assert_eq!(module.description, "Get a user by ID");
        assert_eq!(module.target, "axum::get_user");
        assert!(module.annotations.as_ref().unwrap().readonly);
        assert!(module.annotations.as_ref().unwrap().cacheable);
    }

    #[test]
    fn test_metadata_to_module_no_tags() {
        let scanner = NativeAxumScanner::new();
        let meta = RouteMetadata {
            method: "POST".into(),
            path: "/api/tasks".into(),
            handler_name: "create_task".into(),
            description: "Create a task".into(),
            tags: vec![],
            input_schema: json!({}),
            output_schema: json!({}),
            documentation: None,
        };
        let module = scanner.metadata_to_module(&meta);
        assert_eq!(module.module_id, "tasks.create_task.post");
    }

    #[test]
    fn test_extract_tag_from_path() {
        assert_eq!(extract_tag_from_path("/api/users/:id"), "users");
        assert_eq!(extract_tag_from_path("/api/v1/tasks"), "tasks");
        assert_eq!(extract_tag_from_path("/health"), "health");
        assert_eq!(extract_tag_from_path("/"), "default");
    }

    #[test]
    fn test_scan_routes_with_multiple() {
        let scanner = NativeAxumScanner::new();
        let routes = vec![
            sample_route(),
            RouteMetadata {
                method: "DELETE".into(),
                path: "/api/users/:id".into(),
                handler_name: "delete_user".into(),
                description: "Delete a user".into(),
                tags: vec!["users".into()],
                input_schema: json!({}),
                output_schema: json!({}),
                documentation: None,
            },
        ];
        let modules = scanner.scan_routes(&routes, None, None).unwrap();

        assert_eq!(modules.len(), 2);
        assert_eq!(modules[0].module_id, "users.get_user.get");
        assert_eq!(modules[1].module_id, "users.delete_user.delete");

        // Verify annotations
        assert!(modules[0].annotations.as_ref().unwrap().readonly);
        assert!(modules[1].annotations.as_ref().unwrap().destructive);
    }

    #[test]
    fn test_scan_routes_with_include_filter() {
        let scanner = NativeAxumScanner::new();
        let routes = vec![
            sample_route(),
            RouteMetadata {
                method: "GET".into(),
                path: "/api/tasks".into(),
                handler_name: "list_tasks".into(),
                description: "List tasks".into(),
                tags: vec!["tasks".into()],
                input_schema: json!({}),
                output_schema: json!({}),
                documentation: None,
            },
        ];
        let modules = scanner.scan_routes(&routes, Some("tasks"), None).unwrap();

        assert_eq!(modules.len(), 1);
        assert_eq!(modules[0].module_id, "tasks.list_tasks.get");
    }

    #[test]
    fn test_scan_routes_empty() {
        let scanner = NativeAxumScanner::new();
        let modules = scanner.scan_routes(&[], None, None).unwrap();
        assert!(modules.is_empty());
    }

    #[test]
    fn test_metadata_http_method_in_metadata() {
        let scanner = NativeAxumScanner::new();
        let meta = sample_route();
        let module = scanner.metadata_to_module(&meta);
        assert_eq!(module.metadata.get("http_method").unwrap(), "GET");
        assert_eq!(module.metadata.get("url_path").unwrap(), "/api/users/:id");
    }
}

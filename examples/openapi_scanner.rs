//! OpenAPI scanner example — scan an OpenAPI 3.x document (e.g. utoipa output).
//!
//! Demonstrates:
//! 1. Building an OpenAPI document (simulating `serde_json::to_value(ApiDoc::openapi())`)
//! 2. Scanning it directly with `OpenAPIScanner::scan_spec()`, with include/exclude filters
//! 3. Binding it to a scanner (`get_scanner_with_spec`) to scan through `AxumScanner`
//! 4. The `openapi` source end to end: `APCORE_OPENAPI_SPEC`-style settings, `init_app`,
//!    handlers bound by `target`, and `call_anonymous`
//!
//! Run with: `cargo run --example openapi_scanner --features openapi`

use std::sync::Arc;

use axum::Router;
use serde_json::{json, Value};

use axum_apcore::{
    get_scanner_with_spec, ApcoreSettings, AxumApcore, Context, ModuleError, OpenAPIScanner,
};

// ---------------------------------------------------------------------------
// The document
// ---------------------------------------------------------------------------

/// A pet store document as utoipa would generate it, plus one operation
/// without an operationId (`/health`) and one deprecated operation.
fn pet_store_spec() -> Value {
    let pet = json!({
        "type": "object",
        "properties": {
            "id": {"type": "integer"},
            "name": {"type": "string"},
            "species": {"type": "string"}
        }
    });
    json!({
        "openapi": "3.1.0",
        "info": {"title": "Pet Store API", "version": "1.2.0"},
        "paths": {
            "/api/pets": {
                "get": {
                    "operationId": "list_pets_get",
                    "summary": "List all pets",
                    "tags": ["pets"],
                    "parameters": [{"name": "limit", "in": "query", "schema": {"type": "integer"}}],
                    "responses": {"200": {"description": "A list of pets", "content": {
                        "application/json": {"schema": {"type": "array", "items": pet}}}}}
                },
                "post": {
                    "operationId": "create_pet_post",
                    "summary": "Create a pet",
                    "tags": ["pets"],
                    "requestBody": {"content": {"application/json": {"schema": {
                        "type": "object",
                        "properties": {"name": {"type": "string"}, "species": {"type": "string"}},
                        "required": ["name", "species"]
                    }}}},
                    "responses": {"201": {"description": "Pet created", "content": {
                        "application/json": {"schema": pet}}}}
                }
            },
            "/api/pets/{id}": {
                "get": {
                    "operationId": "get_pet_get",
                    "summary": "Get a pet by ID",
                    "description": "Returns a single pet by its unique identifier.",
                    "tags": ["pets"],
                    "parameters": [{"name": "id", "in": "path", "required": true,
                                    "schema": {"type": "integer"}}],
                    "responses": {"200": {"description": "A pet", "content": {
                        "application/json": {"schema": pet}}}}
                },
                "delete": {
                    "operationId": "delete_pet_delete",
                    "summary": "Delete a pet",
                    "tags": ["pets"],
                    "deprecated": true,
                    "parameters": [{"name": "id", "in": "path", "required": true,
                                    "schema": {"type": "integer"}}],
                    "responses": {"204": {"description": "Pet deleted"}}
                }
            },
            "/api/owners": {
                "get": {
                    "operationId": "list_owners_get",
                    "summary": "List all owners",
                    "tags": ["owners"],
                    "responses": {"200": {"description": "A list of owners"}}
                }
            },
            "/health": {
                "get": {"summary": "Health check", "responses": {"200": {"description": "OK"}}}
            }
        }
    })
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn get_pet(input: Value, _ctx: &Context<Value>) -> Result<Value, ModuleError> {
    Ok(json!({"id": input["id"], "name": "Rex", "species": "dog"}))
}

async fn health(_input: Value, _ctx: &Context<Value>) -> Result<Value, ModuleError> {
    Ok(json!({"status": "ok"}))
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    let spec = pet_store_spec();

    // --- 1. Direct scan (synchronous) ---
    println!("=== Scan all operations ===");
    let scanner = OpenAPIScanner::new();
    let modules = scanner.scan_spec(&spec, None, None).unwrap();
    for m in &modules {
        let annotations = m.annotations.as_ref().unwrap();
        println!(
            "  {:<28} target={:<26} readonly={} deprecated={}",
            m.module_id,
            m.target,
            annotations.readonly,
            annotations.extra.contains_key("deprecated")
        );
    }
    println!("Total: {} modules\n", modules.len());

    println!("=== Filter: include 'pets', exclude 'delete' ===");
    let filtered = scanner
        .scan_spec(&spec, Some("pets"), Some("delete"))
        .unwrap();
    for m in &filtered {
        println!("  {}", m.module_id);
    }
    println!();

    // `simplify_ids = false` keeps the raw operationId in the ID
    // (`pets.list_pets_get.get`); the default strips the `_{method}` suffix.
    println!("=== Without method-suffix stripping ===");
    let raw = OpenAPIScanner::with_simplify_ids(false)
        .scan_spec(&spec, Some("pets"), None)
        .unwrap();
    for m in &raw {
        println!("  {}", m.module_id);
    }
    println!();

    // --- 2. Bound document, scanned through the AxumScanner trait ---
    println!("=== Bound document via get_scanner_with_spec ===");
    let bound = get_scanner_with_spec("openapi", Some(spec.clone())).unwrap();
    let modules = bound
        .scan(&Router::new(), Some("owners"), None)
        .await
        .unwrap();
    println!(
        "  '{}' scanner found: {:?}\n",
        bound.source_name(),
        modules.iter().map(|m| &m.module_id).collect::<Vec<_>>()
    );

    // --- 3. The `openapi` source end to end ---
    // Equivalent to APCORE_SCANNER_SOURCE=openapi APCORE_OPENAPI_SPEC=<path>.
    let spec_path = std::env::temp_dir().join("axum_apcore_openapi_example.yaml");
    std::fs::write(&spec_path, serde_yaml::to_string(&spec).unwrap()).unwrap();
    let settings = ApcoreSettings {
        auto_discover: false,
        scanner_source: "openapi".into(),
        openapi_spec: Some(spec_path.clone()),
        ..ApcoreSettings::default()
    };
    settings.validate().expect("valid settings");

    let apcore = AxumApcore::with_settings(settings);
    // Handlers bind to a module's `target`: `axum::{operationId}`, or the
    // `"GET /path"` route descriptor for an operation without an operationId.
    apcore.register_handler(
        "axum::get_pet_get",
        Arc::new(|input, ctx| Box::pin(get_pet(input, ctx))),
    );
    apcore.register_handler(
        "GET /health",
        Arc::new(|input, ctx| Box::pin(health(input, ctx))),
    );
    apcore.init_app(&Router::new()).await.unwrap();

    println!("=== Calls through init_app ===");
    let pet = apcore
        .call_anonymous("pets.get_pet.get", json!({"id": 7}))
        .await
        .unwrap();
    println!("  pets.get_pet.get -> {pet}");
    let status = apcore
        .call_anonymous("health.get", json!({}))
        .await
        .unwrap();
    println!("  health.get       -> {status}");

    let _ = std::fs::remove_file(&spec_path);
}

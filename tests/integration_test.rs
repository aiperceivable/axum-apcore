// Integration tests for axum-apcore.
//
// Tests the end-to-end flow: metadata registration → scanning → module
// registration → execution, plus task management and context extraction.
//
// NOTE: Tests share global singletons (ROUTE_REGISTRY, REGISTRY, EXECUTOR).
// Each test uses unique handler names to avoid interference.

use axum::http::Request;
use axum::Router;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

use axum_apcore::scanner::native::{register_route, RouteMetadata};
use axum_apcore::{
    ApcoreSettings, AxumApcore, Context, Identity, ModuleError, NativeAxumScanner, RequestIdentity,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn test_settings() -> ApcoreSettings {
    ApcoreSettings {
        auto_discover: false,
        ..ApcoreSettings::default()
    }
}

// D7-001: cross-integration config-default consistency. axum-apcore mirrors
// fastapi-apcore / django-apcore defaults, with the single documented exception
// that serve_transport is "streamable-http" (axum is HTTP-native; stdio N/A).
#[test]
fn test_default_settings_match_sibling_integrations() {
    let s = ApcoreSettings::default();
    // Aligned with fastapi-apcore / django-apcore.
    assert!(
        !s.explorer_enabled,
        "explorer must default off, matching fastapi/django"
    );
    assert_eq!(
        s.task_max_tasks, 1000,
        "task_max_tasks must match fastapi/django default 1000"
    );
    assert_eq!(
        s.serve_port, 9090,
        "serve_port must match fastapi/django default 9090"
    );
    assert_eq!(s.task_max_concurrent, 10);
    // Documented intentional HTTP-native delta.
    assert_eq!(
        s.serve_transport, "streamable-http",
        "axum is HTTP-native, stdio N/A"
    );
}

fn make_route(method: &str, path: &str, handler: &str, desc: &str) -> RouteMetadata {
    RouteMetadata {
        method: method.into(),
        path: path.into(),
        handler_name: handler.into(),
        description: desc.into(),
        tags: vec!["inttest".into()],
        input_schema: json!({"type": "object", "properties": {"id": {"type": "string"}}}),
        output_schema: json!({"type": "object", "properties": {"result": {"type": "string"}}}),
        documentation: None,
    }
}

async fn echo_handler(input: Value, _ctx: &Context<Value>) -> Result<Value, ModuleError> {
    Ok(json!({"echo": input}))
}

async fn ctx_echo_handler(input: Value, ctx: &Context<Value>) -> Result<Value, ModuleError> {
    Ok(json!({
        "caller": ctx.identity.as_ref().map(|i| i.id()).unwrap_or("anonymous"),
        "input": input,
    }))
}

async fn slow_handler(input: Value, ctx: &Context<Value>) -> Result<Value, ModuleError> {
    let ms = input["ms"].as_u64().unwrap_or(500);
    for _ in 0..ms / 10 {
        if let Some(token) = &ctx.cancel_token {
            if token.is_cancelled() {
                return Err(ModuleError::new(
                    axum_apcore::ErrorCode::ExecutionCancelled,
                    "cancelled".to_string(),
                ));
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Ok(json!({"slept_ms": ms}))
}

// ---------------------------------------------------------------------------
// End-to-end: register → scan → call
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_e2e_register_scan_call() {
    register_route(make_route(
        "GET",
        "/api/e2e_items/:id",
        "e2e_get_item",
        "Get item (e2e)",
    ));

    let apcore = AxumApcore::with_settings(test_settings());
    apcore.register_handler(
        "axum::e2e_get_item",
        Arc::new(|input, ctx| Box::pin(echo_handler(input, ctx))),
    );

    let router = Router::new();
    apcore.init_app(&router).await.unwrap();

    let modules = apcore.list_modules();
    assert!(
        modules.iter().any(|m| m.contains("e2e_get_item")),
        "Expected e2e_get_item module, got: {modules:?}"
    );

    let result = apcore
        .call_anonymous("inttest.e2e_get_item.get", json!({"id": "42"}))
        .await
        .unwrap();

    assert_eq!(result["echo"]["id"], "42");
}

// ---------------------------------------------------------------------------
// Call with explicit context
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_call_with_identity_context() {
    register_route(make_route(
        "GET",
        "/api/ctx_test",
        "ctx_test_handler",
        "Context test",
    ));

    let apcore = AxumApcore::with_settings(test_settings());
    apcore.register_handler(
        "axum::ctx_test_handler",
        Arc::new(|input, ctx| Box::pin(ctx_echo_handler(input, ctx))),
    );

    let router = Router::new();
    apcore.init_app(&router).await.unwrap();

    let ctx = Context::new(Identity::new(
        "admin-ctx".into(),
        "admin".into(),
        vec!["admin".into()],
        Default::default(),
    ));

    let result = apcore
        .call(
            "inttest.ctx_test_handler.get",
            json!({"key": "val"}),
            Some(&ctx),
        )
        .await
        .unwrap();

    assert_eq!(result["caller"], "admin-ctx");
    assert_eq!(result["input"]["key"], "val");
}

// ---------------------------------------------------------------------------
// Cancellable call — timeout
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_cancellable_call_timeout() {
    register_route(make_route(
        "POST",
        "/api/slow_cancel",
        "slow_cancel_op",
        "Slow cancel op",
    ));

    let apcore = AxumApcore::with_settings(test_settings());
    apcore.register_handler(
        "axum::slow_cancel_op",
        Arc::new(|input, ctx| Box::pin(slow_handler(input, ctx))),
    );

    let router = Router::new();
    apcore.init_app(&router).await.unwrap();

    let result = apcore
        .cancellable_call(
            "inttest.slow_cancel_op.post",
            json!({"ms": 5000}),
            None,
            Duration::from_millis(100),
        )
        .await;

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("timed out"),
        "Expected timeout error, got: {err}"
    );
}

// ---------------------------------------------------------------------------
// Task management: submit → complete → result
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_task_submit_complete_and_result() {
    register_route(make_route(
        "POST",
        "/api/task_echo",
        "task_echo_op",
        "Task echo",
    ));

    let apcore = AxumApcore::with_settings(test_settings());
    apcore.register_handler(
        "axum::task_echo_op",
        Arc::new(|input, ctx| Box::pin(echo_handler(input, ctx))),
    );

    let router = Router::new();
    apcore.init_app(&router).await.unwrap();

    let task_id = apcore
        .submit_task("inttest.task_echo_op.post", json!({"data": "hello"}))
        .unwrap();

    // Poll until complete (max 2s)
    let mut completed = false;
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if let Some(info) = apcore.get_task_status(&task_id) {
            if info.status != "Running" {
                completed = true;
                break;
            }
        }
    }
    assert!(completed, "Task did not complete within 2s");

    let result = apcore.get_task_result(&task_id);
    assert!(result.is_some(), "Task result should be available");
    assert_eq!(result.unwrap()["echo"]["data"], "hello");
}

// ---------------------------------------------------------------------------
// Task management: cancel
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_task_cancel() {
    register_route(make_route(
        "POST",
        "/api/task_slow",
        "task_slow_op",
        "Slow task",
    ));

    let apcore = AxumApcore::with_settings(test_settings());
    apcore.register_handler(
        "axum::task_slow_op",
        Arc::new(|input, ctx| Box::pin(slow_handler(input, ctx))),
    );

    let router = Router::new();
    apcore.init_app(&router).await.unwrap();

    let task_id = apcore
        .submit_task("inttest.task_slow_op.post", json!({"ms": 5000}))
        .unwrap();

    // Give it time to start
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        apcore.cancel_task(&task_id),
        "Should be able to cancel a running task"
    );

    let info = apcore.get_task_status(&task_id).unwrap();
    assert_eq!(info.status, "Cancelled");
}

// ---------------------------------------------------------------------------
// Scanner: include/exclude filters (no global state)
// ---------------------------------------------------------------------------

#[test]
fn test_scanner_include_exclude() {
    let scanner = NativeAxumScanner::new();
    let routes = vec![
        make_route("GET", "/api/users/:id", "scan_get_user", "Get user"),
        make_route("POST", "/api/tasks", "scan_create_task", "Create task"),
        make_route(
            "DELETE",
            "/api/users/:id",
            "scan_delete_user",
            "Delete user",
        ),
    ];

    // Include only users
    let filtered = scanner.scan_routes(&routes, Some("user"), None).unwrap();
    assert_eq!(filtered.len(), 2);

    // Exclude delete
    let filtered = scanner.scan_routes(&routes, None, Some("delete")).unwrap();
    assert_eq!(filtered.len(), 2);

    // Include users + exclude delete
    let filtered = scanner
        .scan_routes(&routes, Some("user"), Some("delete"))
        .unwrap();
    assert_eq!(filtered.len(), 1);
    assert!(filtered[0].module_id.contains("scan_get_user"));
}

// ---------------------------------------------------------------------------
// Context extraction from request parts
// ---------------------------------------------------------------------------

#[test]
fn test_context_factory_with_request_identity() {
    let factory = axum_apcore::AxumContextFactory;

    let mut req = Request::builder().body(()).unwrap();
    req.extensions_mut().insert(RequestIdentity {
        id: "svc-1".into(),
        identity_type: "service".into(),
        roles: vec!["reader".into(), "writer".into()],
        attrs: Default::default(),
    });
    let (parts, _) = req.into_parts();

    let ctx = factory.create_from_parts(&parts).unwrap();
    let identity = ctx.identity.as_ref().unwrap();
    assert_eq!(identity.id(), "svc-1");
    assert_eq!(identity.identity_type(), "service");
    assert_eq!(identity.roles().len(), 2);
}

#[test]
fn test_context_factory_anonymous_fallback() {
    let factory = axum_apcore::AxumContextFactory;

    let req = Request::builder().body(()).unwrap();
    let (parts, _) = req.into_parts();

    let ctx = factory.create_from_parts(&parts).unwrap();
    assert_eq!(ctx.identity.as_ref().unwrap().id(), "anonymous");
}

// ---------------------------------------------------------------------------
// OpenAPI scanner (feature-gated)
// ---------------------------------------------------------------------------

#[cfg(feature = "openapi")]
#[test]
fn test_openapi_scanner_end_to_end() {
    let scanner = axum_apcore::OpenAPIScanner::new();
    let spec = json!({
        "openapi": "3.1.0",
        "info": {"title": "Test", "version": "1.0.0"},
        "paths": {
            "/items": {
                "get": {
                    "operationId": "list_items_get",
                    "summary": "List items",
                    "tags": ["items"],
                    "responses": {"200": {"description": "OK"}}
                }
            }
        }
    });

    let modules = scanner.scan_spec(&spec, None, None).unwrap();
    assert_eq!(modules.len(), 1);
    assert_eq!(modules[0].module_id, "items.list_items.get");
    assert!(modules[0].annotations.as_ref().unwrap().readonly);
}

#[cfg(feature = "openapi")]
async fn oa_health_handler(_input: Value, _ctx: &Context<Value>) -> Result<Value, ModuleError> {
    Ok(json!({"route": "health"}))
}

#[cfg(feature = "openapi")]
async fn oa_metrics_handler(_input: Value, _ctx: &Context<Value>) -> Result<Value, ModuleError> {
    Ok(json!({"route": "metrics"}))
}

/// Two operations with an operationId-free path plus one utoipa-style
/// operation, under a path prefix unique to these tests (the registry and
/// executor are process-global).
#[cfg(feature = "openapi")]
fn oa_e2e_spec() -> Value {
    json!({
        "openapi": "3.1.0",
        "info": {"title": "E2E", "version": "3.2.1"},
        "paths": {
            "/oa_e2e/items/{id}": {
                "get": {
                    "operationId": "oa_e2e_get_item_get",
                    "summary": "Get item",
                    "tags": ["oa_e2e"],
                    "parameters": [{"name": "id", "in": "path", "required": true,
                                    "schema": {"type": "string"}}],
                    "responses": {"200": {"description": "OK"}}
                }
            },
            "/oa_e2e/health": {
                "get": {"summary": "Health", "responses": {"200": {"description": "OK"}}}
            },
            "/oa_e2e/metrics": {
                "get": {"summary": "Metrics", "responses": {"200": {"description": "OK"}}}
            }
        }
    })
}

#[cfg(feature = "openapi")]
#[tokio::test]
async fn test_openapi_source_end_to_end_through_init_app() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("openapi.json");
    std::fs::write(&path, oa_e2e_spec().to_string()).unwrap();

    let settings = ApcoreSettings {
        auto_discover: false,
        scanner_source: "openapi".into(),
        openapi_spec: Some(path),
        ..ApcoreSettings::default()
    };
    assert!(settings.validate().is_ok());

    let apcore = AxumApcore::with_settings(settings);
    // `target` is the handler key: `axum::{operationId}` when there is one,
    // the `"GET /path"` route descriptor otherwise.
    apcore.register_handler(
        "axum::oa_e2e_get_item_get",
        Arc::new(|input, ctx| Box::pin(echo_handler(input, ctx))),
    );
    apcore.register_handler(
        "GET /oa_e2e/health",
        Arc::new(|input, ctx| Box::pin(oa_health_handler(input, ctx))),
    );
    apcore.register_handler(
        "GET /oa_e2e/metrics",
        Arc::new(|input, ctx| Box::pin(oa_metrics_handler(input, ctx))),
    );
    apcore.init_app(&Router::new()).await.unwrap();

    let modules = apcore.list_modules();
    for expected in [
        "oa_e2e.oa_e2e_get_item.get",
        "oa_e2e.health.get",
        "oa_e2e.metrics.get",
    ] {
        assert!(
            modules.iter().any(|m| m == expected),
            "missing {expected} in {modules:?}"
        );
    }

    let item = apcore
        .call_anonymous("oa_e2e.oa_e2e_get_item.get", json!({"id": "7"}))
        .await
        .unwrap();
    assert_eq!(item["echo"]["id"], "7");

    // Without an operationId both operations used to collapse onto
    // `default.unknown.get` / `default.unknown.get_2` with one shared target,
    // so only one handler could ever be bound. Each now reaches its own.
    let health = apcore
        .call_anonymous("oa_e2e.health.get", json!({}))
        .await
        .unwrap();
    assert_eq!(health["route"], "health");
    let metrics = apcore
        .call_anonymous("oa_e2e.metrics.get", json!({}))
        .await
        .unwrap();
    assert_eq!(metrics["route"], "metrics");
}

#[cfg(feature = "openapi")]
#[tokio::test]
async fn test_openapi_source_scans_yaml_spec_through_client() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("openapi.yaml");
    let yaml = serde_yaml::to_string(&oa_e2e_spec()).unwrap();
    std::fs::write(&path, yaml).unwrap();

    let apcore = AxumApcore::with_settings(ApcoreSettings {
        auto_discover: false,
        scanner_source: "openapi".into(),
        openapi_spec: Some(path),
        ..ApcoreSettings::default()
    });
    let modules = apcore
        .scan(&Router::new(), Some("health|metrics"), None)
        .await
        .unwrap();
    let mut targets: Vec<&str> = modules.iter().map(|m| m.target.as_str()).collect();
    targets.sort();
    assert_eq!(targets, vec!["GET /oa_e2e/health", "GET /oa_e2e/metrics"]);
    assert!(modules.iter().all(|m| m.version == "3.2.1"));
}

#[cfg(feature = "openapi")]
#[tokio::test]
async fn test_openapi_source_without_spec_is_rejected() {
    let settings = ApcoreSettings {
        auto_discover: false,
        scanner_source: "openapi".into(),
        ..ApcoreSettings::default()
    };
    let errors = settings.validate().unwrap_err();
    assert!(errors.iter().any(|e| e.contains("APCORE_OPENAPI_SPEC")));

    let apcore = AxumApcore::with_settings(settings);
    let err = apcore.init_app(&Router::new()).await.unwrap_err();
    assert!(
        err.to_string().contains("APCORE_OPENAPI_SPEC"),
        "unexpected error: {err}"
    );
}

#[cfg(all(feature = "openapi", feature = "cli"))]
#[tokio::test]
async fn test_openapi_source_reaches_create_cli() {
    let apcore = AxumApcore::with_settings(test_settings());

    let config = axum_apcore::CreateCliConfig {
        scan_source: "openapi".into(),
        openapi_spec: Some(oa_e2e_spec()),
        ..Default::default()
    };
    let cmd = apcore.create_cli(&Router::new(), config).await.unwrap();
    let builtins = ["list", "describe", "completion", "man", "init"];
    let route_commands: Vec<String> = cmd
        .get_subcommands()
        .map(|c| c.get_name().to_string())
        .filter(|name| !builtins.contains(&name.as_str()))
        .collect();
    assert_eq!(
        route_commands.len(),
        3,
        "expected one command per operation, got {route_commands:?}"
    );

    let missing = axum_apcore::CreateCliConfig {
        scan_source: "openapi".into(),
        ..Default::default()
    };
    assert!(apcore.create_cli(&Router::new(), missing).await.is_err());
}

// ---------------------------------------------------------------------------
// Settings validation
// ---------------------------------------------------------------------------

#[test]
fn test_settings_validation_catches_invalid() {
    let settings = ApcoreSettings {
        serve_transport: "grpc".into(),
        scanner_source: "magic".into(),
        serve_port: 0,
        ..ApcoreSettings::default()
    };
    let errors = settings.validate().unwrap_err();
    assert_eq!(errors.len(), 3);
}

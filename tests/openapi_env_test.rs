// The `openapi` scanner source configured purely from APCORE_* environment
// variables, through the global settings singleton.
//
// This file deliberately holds a single test: it mutates the process
// environment, which would race with tests running in parallel threads of
// the same binary (each file under tests/ is its own binary and process).

#![cfg(feature = "openapi")]

use axum::Router;
use axum_apcore::{ApcoreSettings, AxumApcore};

#[tokio::test]
async fn test_openapi_source_configured_from_env() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("openapi.yaml");
    std::fs::write(
        &path,
        "openapi: 3.0.3\n\
         info: {title: Env, version: 0.9.0}\n\
         paths:\n  \
           /env_items:\n    \
             get:\n      \
               operationId: env_list_items_get\n      \
               tags: [env]\n      \
               responses:\n        \
                 '200': {description: OK}\n",
    )
    .unwrap();

    std::env::set_var("APCORE_SCANNER_SOURCE", "openapi");
    std::env::set_var("APCORE_OPENAPI_SPEC", &path);
    std::env::set_var("APCORE_AUTO_DISCOVER", "false");

    let settings = ApcoreSettings::from_env();
    assert_eq!(settings.scanner_source, "openapi");
    assert_eq!(settings.openapi_spec.as_deref(), Some(path.as_path()));
    assert!(settings.validate().is_ok());

    // `AxumApcore::new()` reads the same variables via the global settings.
    let apcore = AxumApcore::new();
    apcore.init_app(&Router::new()).await.unwrap();
    assert!(apcore
        .list_modules()
        .iter()
        .any(|m| m == "env.env_list_items.get"));
}

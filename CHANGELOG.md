# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---
## [Unreleased]

Planned as 0.4.0. The `openapi` scanner source becomes reachable from every entry point, and OpenAPI traversal is delegated to `apcore_toolkit::OpenAPIScanner` (apcore-toolkit 0.12).

### Breaking changes

- **`get_scanner("openapi")` returns `Err`.** It used to return a scanner whose every `scan()` failed. A source name cannot supply a document; use `get_scanner_with_spec("openapi", Some(spec))`.
- **`ApcoreSettings::validate()` is stricter for the `openapi` source.** It rejects `scanner_source = "openapi"` unless `openapi_spec` names an existing file. When the `openapi` feature is disabled it rejects `"openapi"` outright; before, the value was accepted and then failed at scan time.
- **New public fields.** `ApcoreSettings::openapi_spec`, `CreateCliConfig::openapi_spec` and `cli::Commands::Scan::spec` are new. A struct literal or exhaustive pattern that lists every field must add them; the `..Default::default()` form is unaffected.
- **`OpenAPIScanner` has a private field** (the bound document), so a struct literal (`OpenAPIScanner { simplify_ids }`) no longer compiles. Construct it with `new()`, `with_simplify_ids()` or `with_spec()`. `simplify_ids` stays a public field.
- **OpenAPI scan output changes.** These come from delegating to the toolkit. Operations *with* an operationId keep their `module_id`, `target` and `suggested_alias`.
  - An operation **without an operationId** gets the toolkit's path-derived `module_id` (`health.get`, `users.id.get`) and the target `"GET /path"`. Before, it got `default.unknown.<method>` (then `_2`, `_3`, ...) and every such operation shared the target `axum::unknown`. An empty `operationId: ""` now counts as absent; before, it was used verbatim (`users..get`).
  - **Swagger 2.0** documents, and any document without an `openapi: 3.0.x` / `3.1.x` key, are rejected with `AxumApcoreError::Scanner`. Before, they were scanned.
  - A document **without `paths`** yields an empty list. Before, it was an error.
  - `deprecated: true` is recorded as `annotations.extra["deprecated"] = true`. Before, it was ignored.
  - `version` comes from `info.version`. Before, it was hard-coded to `"1.0.0"`, which is now only the fallback.
  - `description` is the summary, or else the **first non-empty line** of the description. Before, it was the whole description, which `documentation` still carries.
  - `metadata.openapi` is new and records `spec_version`, `operation_id`, `summary`, and `server_url` when resolvable. `warnings` now reports unresolvable or external `$ref`s and a missing 2xx response.
  - include/exclude filters now run before ID deduplication (the toolkit's order). A pattern can therefore no longer select a `_2` dedup suffix.
  - **Resolved against apcore-toolkit's next release (0.13), IDs apcore could not register are normalised.** The toolkit will pass every emitted ID through its Canonical-ID normalisation, so an axum ID with uppercase letters — `Items.replaceItemSub.put`, from a capitalised tag or a camelCase operationId — becomes `items.replace_item_sub.put`, and include/exclude patterns match that spelling. No working ID changes: a legal ID is returned unchanged, and an uppercase one never registered. The `apcore-toolkit = ">=0.12"` bound picks the release up automatically; the test suite passes against both 0.12.0 and the unreleased toolkit.

### Added

- **`OpenAPIScanner::with_spec(spec)`** binds an OpenAPI document at construction; `AxumScanner::scan` scans it and ignores the `Router`. `OpenAPIScanner::spec()` returns the bound document. `OpenAPIScanner` now derives `Debug` and `Clone`.
- **`get_scanner_with_spec(source, Option<serde_json::Value>)`** builds a scanner by source name and binds the document for `"openapi"`. It is re-exported at the crate root.
- **`scanner::openapi::load_spec_file(&Path)`** loads a local JSON or YAML OpenAPI document. The format is detected from the content.
- **`ApcoreSettings::openapi_spec` / `APCORE_OPENAPI_SPEC`** give the document path used by `AxumApcore::scan` / `init_app` when `APCORE_SCANNER_SOURCE=openapi`.
- **CLI `scan --spec <PATH>`** gives the document for `--source openapi`. It defaults to `APCORE_OPENAPI_SPEC`.
- **`CreateCliConfig::openapi_spec`** is the document `create_cli` scans when `scan_source` is `"openapi"`.

### Changed

- **`OpenAPIScanner` delegates to `apcore_toolkit::OpenAPIScanner`.** axum-apcore's naming is kept through the toolkit's hooks. The `derive_module_id` hook keeps `{tag}.{func}.{method}` for operations with an operationId and otherwise falls back to the toolkit's path derivation. The `transform_module` hook keeps `target = axum::{operationId}` when there is one, plus `suggested_alias`. The scanner's own traversal, `is_http_method`, and the `"unknown"` operationId fallback are gone.
- **`scan_spec` stays synchronous.** It drives the toolkit's `async fn scan` with `futures::FutureExt::now_or_never`. That call polls exactly once and never parks the thread, so it cannot deadlock, even on a current-thread Tokio runtime. The toolkit 0.12 scan has no suspension point. If a later toolkit version adds one, `scan_spec` returns a `Scanner` error rather than hanging.
- **Toolkit errors map onto `AxumApcoreError`.** An invalid include/exclude regex stays `AxumApcoreError::Regex`, as before. Every other `ScannerError` becomes `AxumApcoreError::Scanner`.
- **`apcore-toolkit >= 0.12`** (was `>= 0.10`).
- **`ScannedModule` is built with `ScannedModule::new` plus field assignment**, in `NativeAxumScanner` and the OpenAPI hooks, instead of a 14-field struct literal. The struct is not `#[non_exhaustive]` and the toolkit bound is open-ended, so a field the toolkit adds no longer breaks the build. The native scanner's output is unchanged.
- **`OpenAPIScanner::simplify_ids` is documented as a misnomer.** It only strips the trailing `_{method}` from an operationId, which is what fastapi-apcore's *default* (`simplify_ids=False`) does. It is not the deprecated display simplification of apcore PROTOCOL_SPEC §5.13.11, so the defaults are unchanged and there is no deprecation warning.
- **`AxumScanner` docs corrected.** They claimed the trait wraps an apcore-toolkit scanner trait and that the native scanner introspects the `Router`; neither is true. `AxumScanner` is axum-apcore's own trait, carrying the include/exclude filters on every call, and both scanners ignore the type-erased `Router`. It stays off `apcore_toolkit::BaseScanner` because nothing in the ecosystem dispatches over that trait. Migrating is possible once apcore-toolkit-rust makes `BaseScanner::scan` fallible, but is out of scope here.
- **`[lints.clippy] result_large_err = "allow"`** is now set package-wide. Handlers must return `Result<Value, ModuleError>`, and current clippy flags that return type in every test and example handler, so `cargo clippy --all-targets --all-features -- -D warnings` failed on 0.3.0 as well. `src/lib.rs` already allowed the lint for the library.

### Fixed

- **The `openapi` scanner source was a dead path.** `get_scanner("openapi")` returned a scanner whose `AxumScanner::scan` always failed, because a `Router` carries no OpenAPI document. As a result `init_app` / `scan` with `APCORE_SCANNER_SOURCE=openapi`, `axum-apcore scan --source openapi` and `create_cli` with `scan_source: "openapi"` all failed. Each now works with a document supplied as described above.
- **Operations without an operationId collided.** They collapsed onto `default.unknown.<method>` / `..._2` with one shared `target`, so only one handler could be bound. Each now gets a distinct `module_id` and `target`.
- **README:** `APCORE_TASK_MAX_TASKS` defaults to `1000`; it was documented as `100`.

### Removed

- **The optional `utoipa` dependency.** Nothing used it. The `openapi` feature is kept, now pulling in no extra crate, so `features = ["openapi"]` still builds. utoipa apps pass `serde_json::to_value(ApiDoc::openapi())`.

---
## [0.3.0] - 2026-07-16

ACL demo + dependency uplift to the aligned apcore 0.26.0 / apcore-mcp 0.17.2 governance train.

### Added

- **ACL demo (`examples/acl_demo/`)** — runnable Axum app showing apcore Access Control List enforcement on route handlers, matching the shared cross-integration contract: an `acl.yaml` (admins may call anything; `orders.list` public; else denied), `orders.delete` / `orders.list` registered as apcore modules with explicit IDs, an `inject_identity` middleware mapping a comma-separated `X-Roles` header into a `RequestIdentity` extension, and handlers that call the modules through `AxumApcore::call()` with `ACLDenied` mapped to HTTP 403. Verified by `cargo test --example acl_demo` (admin allowed; anonymous / non-admin denied; public read).

### Changed

- **Dependency floors raised to the aligned governance train and loosened from caret to `>=`** (matching the first-party apcore-dependency convention, so future apcore minor releases need no downstream edits): `apcore >= 0.26` (was `0.25`), `apcore-toolkit >= 0.10` (was `0.9`), `apcore-cli >= 0.10.4`, `apcore-mcp >= 0.17.2`.

---
## [0.2.0] - 2026-06-30

### Added

#### Access Control (ACL)
- **ACL enforcement via `acl_path`** — When `APCORE_ACL_PATH` (settings `acl_path`) points at an ACL YAML file, the executor is now built with that ACL attached (`Executor::with_options`), so apcore's `acl_check` pipeline step authorizes every `call()` / `stream()`. Top-level (HTTP-originated) calls are checked as the `@external` caller; inter-module calls are checked under the calling module's id. Mirrors fastapi-apcore's `acl_path`-driven wiring.
- **`tests/acl_test.rs`** — Integration test that loads an ACL fixture and asserts a denied module returns `ErrorCode::ACLDenied` while an allowed module succeeds.

### Changed

#### Dependency Upgrades
- **`apcore`** 0.24 → 0.25 — `Executor` is now fully interior-mutable (`call`/`stream` take `&self`; `registry` is an `Arc<Registry>`).
- **`apcore-toolkit`** 0.8 → 0.9.1 — `RegistryWriter::write` / `HttpProxyWriter::write` relaxed from `&mut Registry` to `&Registry`.
- **`apcore-mcp`** 0.16 → 0.17 — `MCPServer` now actually serves: drives the transport, registers tool/resource handlers, mounts `/mcp`, and wires approvals.
- **`apcore-cli`** 0.10.1 → 0.10.2.

#### Executor
- **Lock-free executor** — The global executor is now a bare `Arc<Executor>` (was `Arc<tokio::sync::Mutex<Executor>>`). apcore 0.25's interior-mutable `Executor` lets concurrent callers share one instance without serializing through a mutex, removing a per-call bottleneck in `call`/`stream`/`cancellable_call`/`submit_task`/`register_modules`.

#### MCP
- **`create_mcp_server()` wires the live executor** — Builds the server via `MCPServer::with_registry_or_executor(RegistryOrExecutor::Executor(...), config)` so MCP tool calls execute real registered handlers, and spreads apcore-mcp 0.17's new config fields (`trace`, `explorer`, `explorer_prefix`, …) from `ApcoreSettings`.

#### Errors
- **`ACLDenied` → HTTP 403** — `AxumApcoreError::into_response` now maps an ACL denial to `403 Forbidden` instead of the generic `500` (other execution errors stay `500`).

### Fixed
- **`acl_path` was a dead config** — The `acl_path` setting and `APCORE_ACL_PATH` env var were parsed but never loaded or applied, so ACL was silently a no-op. The path is now loaded via `ACL::load` and enforced by the executor. A malformed or missing ACL file is logged and treated as "no ACL" rather than crashing executor initialization.

### Tests
- 80 unit + 11 integration + 1 ACL test, all passing with `cargo test --all-features`.

### Added

#### apcore-cli Integration (feature = `cli`)
- **`create_cli()`** — New method on `AxumApcore` that scans Axum routes, registers them as HTTP proxy modules via `HTTPProxyRegistryWriter`, and builds a grouped clap `Command` using apcore-cli's `GroupedModuleGroup`. Mirrors fastapi-apcore's `create_cli()` pattern.
- **`CreateCliConfig`** — Configuration struct for `create_cli()` with prog_name, base_url, auth_header_factory, timeout, scan_source, include/exclude filters, help_text_max_length, docs_url, and verbose_help.
- **`list` command** — List available modules in the registry, delegated to `apcore_cli::cmd_list`. Supports `--tag` filtering and `--format` (table/json).
- **`describe` command** — Show schema and annotations for a module, delegated to `apcore_cli::cmd_describe`.
- **`completion` command** — Generate shell completion scripts (bash, zsh, fish, elvish, powershell), delegated to `apcore_cli::cmd_completion`.
- **`man` command** — Generate roff man pages for any command, delegated to `apcore_cli::cmd_man` and `apcore_cli::build_program_man_page`.
- **`init module` command** — Scaffold new apcore module files (decorator, convention, or binding style), delegated to `apcore_cli::handle_init`.
- **`cli_proxy` example** — New example demonstrating HTTP proxy CLI generation with `create_cli()`.

#### Re-exports
- **`HTTPProxyRegistryWriter`** — Re-exported from apcore-toolkit when `cli` feature is enabled.
- **`CreateCliConfig`** — Re-exported at crate root when `cli` feature is enabled.

### Changed

#### Dependency Upgrades
- **`apcore`** 0.14 → 0.15 — `Context.identity` changed from `Identity` to `Option<Identity>`; all production code and tests updated.
- **`apcore-toolkit`** 0.3 → 0.4 — Adds `DisplayResolver`, `SyntaxVerifier`, and `HTTPProxyRegistryWriter` (http-proxy feature).
- **`apcore-mcp`** 0.10 → 0.12 — Adds MCP Explorer, error formatter integration, identity propagation, and display overlays.

#### CLI Feature Expansion
- The `cli` feature now includes `apcore-cli` (0.5), `clap_complete`, and enables `apcore-toolkit/http-proxy` for HTTP proxy module support.
- CLI description updated from "scan routes, serve MCP, and export tools" to "scan routes, serve MCP, export tools, and manage modules".

### Tests
- 79 unit tests + 10 integration tests (89 total), all passing with `cargo test --all-features`
- Added 6 new CLI tests: `test_build_registry_provider_empty`, `test_run_list_empty_registry`, `test_run_completion_bash`, `test_run_completion_invalid_shell`, `test_run_man_program_page`, `test_run_man_unknown_command`
- Added 6 CLI parsing tests: list, list_with_tags, describe, completion, man, init_module

---

## [0.1.1] - 2026-03-22

### Changed
- Rebrand: aipartnerup → aiperceivable


## [0.1.0] - 2026-03-20

Initial release. Axum integration for the apcore AI-Perceivable Core ecosystem,
feature-aligned with [fastapi-apcore](https://github.com/aiperceivable/fastapi-apcore).

### Added

#### Core
- **`AxumApcore`** — Unified entry point: init, scan, register, call, stream, export
- **`ApcoreSettings`** — Configuration from `APCORE_*` environment variables with validation
- **`ap_handler!` macro** — Declarative route metadata registration at compile time
- **`AxumApcoreError`** — `thiserror`-based error enum with `IntoResponse` for Axum handlers

#### Context Extraction
- **`ApContext`** — Axum `FromRequestParts` extractor for apcore `Context<Value>`
- **`RequestIdentity`** — Identity struct for auth middleware to inject into request extensions
- **`AxumContextFactory`** — Creates apcore contexts from Axum request parts with W3C `traceparent` support

#### Scanning
- **`NativeAxumScanner`** — Scans routes from the compile-time metadata registry (`RouteMetadata`)
- **`OpenAPIScanner`** — Scans routes from utoipa-generated OpenAPI specs (feature = `openapi`)
- **`AxumScanner` trait** — Extensible scanner interface with include/exclude regex filters
- **`get_scanner()`** — Factory function for scanner selection by source name

#### Execution
- **`call()`** — Execute a module by ID with optional context
- **`call_anonymous()`** — Execute with a default anonymous identity
- **`stream()`** — Execute with streaming output (vec-wrapped)
- **`cancellable_call()`** — Execute with timeout and cooperative cancellation via `CancelToken`
- **`register_handler()`** — Register callable handler functions for target strings
- Executor uses `tokio::sync::Mutex` for safe async lock holding

#### Task Management
- **`TaskManager`** — Async task submission with concurrency and total limits
- **`submit_task()`** — Background execution via `tokio::spawn`
- **`get_task_status()` / `get_task_result()`** — Poll task lifecycle
- **`cancel_task()`** — Cancel running tasks via `CancelToken`
- **`list_tasks()`** — List tasks with optional status filter
- **`cleanup()`** — Remove completed/failed/cancelled tasks by age

#### Engine
- **`get_registry()` / `get_executor()`** — Thread-safe singleton management via `OnceLock`
- **`AxumRegistryWriter`** — Registers scanned modules into both query registry and executor registry
- **`AxumDiscoverer`** — Discovers modules from YAML binding files
- **`AxumModuleValidator`** — Validates module IDs (length, reserved words, segment format)
- **`setup_observability()`** — Configures tracing, metrics, and error history from settings

#### MCP & Export (feature = `mcp`)
- **`create_mcp_server()`** — Create an MCP server from the registry (stdio, streamable-http, SSE)
- **`to_openai_tools()`** — Export modules as OpenAI-compatible tool definitions

#### CLI (feature = `cli`)
- **`scan`** — Scan routes and output to registry or YAML
- **`serve`** — Start an MCP server exposing registered modules
- **`export`** — Export modules as OpenAI tool definitions
- **`tasks`** — List, cancel, and clean up async tasks

#### Examples
- `basic` — Full Axum app with `ap_handler!`, `ApContext`, and server startup
- `handler_registration` — Register handlers, call with `call()` and `call_anonymous()`
- `async_tasks` — Submit, poll, cancel, and list background tasks
- `openapi_scanner` — Scan OpenAPI specs with include/exclude filters
- `mcp_server` — Create MCP server and export OpenAI tools

#### Tests
- 67 unit tests across all modules
- 10 integration tests covering end-to-end flow, task management, context extraction, and scanner filters
- All tests pass with `cargo test --all-features`

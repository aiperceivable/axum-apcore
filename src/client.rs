// AxumApcore — unified entry point for the axum-apcore integration.
//
// This is the main client that ties together scanning, registration,
// context mapping, task management, and MCP serving.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;

use apcore::cancel::CancelToken;
use apcore::context::{Context, Identity};
use apcore::{Executor, Registry};
use apcore_toolkit::{HandlerFn, ScannedModule};

use crate::config::{get_apcore_settings, ApcoreSettings};
use crate::context::AxumContextFactory;
use crate::engine::extensions::{AxumDiscoverer, AxumModuleValidator};
use crate::engine::observability::{setup_observability, ObservabilityState};
use crate::engine::registry::{get_executor, get_registry};
use crate::engine::tasks::{TaskInfo, TaskManager};
use crate::errors::AxumApcoreError;
use crate::output::AxumRegistryWriter;
use crate::scanner::get_scanner_with_spec_path;

/// Main entry point for axum-apcore integration.
///
/// # Usage
///
/// ```ignore
/// use axum_apcore::AxumApcore;
///
/// let apcore = AxumApcore::new();
/// apcore.init_app(&router).await?;
///
/// // Execute a module
/// let result = apcore.call("users.get_user.get", json!({"id": "123"}), None).await?;
/// ```
pub struct AxumApcore {
    settings: ApcoreSettings,
    registry: Arc<Mutex<Registry>>,
    executor: Arc<Executor>,
    context_factory: Arc<AxumContextFactory>,
    task_manager: TaskManager,
    observability: ObservabilityState,
    handler_map: Arc<Mutex<HashMap<String, HandlerFn>>>,
}

impl AxumApcore {
    /// Create a new AxumApcore with default settings from environment.
    pub fn new() -> Self {
        let settings = get_apcore_settings().clone();
        Self::with_settings(settings)
    }

    /// Create a new AxumApcore with explicit settings.
    pub fn with_settings(settings: ApcoreSettings) -> Self {
        let observability = setup_observability(&settings);
        let task_manager = TaskManager::from_settings(&settings);

        Self {
            settings,
            registry: get_registry(),
            executor: get_executor(),
            context_factory: Arc::new(AxumContextFactory),
            task_manager,
            observability,
            handler_map: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    // ---- Lifecycle ----

    /// Initialize the integration: discover modules, scan routes, register.
    ///
    /// This is the equivalent of FastAPIApcore.init_app(). Call it after
    /// building your Axum Router but before serving.
    pub async fn init_app(&self, router: &axum::Router) -> Result<(), AxumApcoreError> {
        // Step 1: Auto-discover modules from binding files
        if self.settings.auto_discover {
            self.discover_modules()?;
        }

        // Step 2: Scan routes and register
        let modules = self.scan(router, None, None).await?;
        self.register_modules(&modules).await?;

        tracing::info!(
            count = modules.len(),
            "axum-apcore initialized with {} modules",
            modules.len()
        );

        Ok(())
    }

    /// Discover modules from YAML binding files.
    fn discover_modules(&self) -> Result<(), AxumApcoreError> {
        let discoverer = AxumDiscoverer::new(self.settings.clone());
        let discovered = discoverer.discover()?;

        let mut registry = self.registry.lock().expect("registry lock poisoned");
        let writer = AxumRegistryWriter::new();

        let scanned: Vec<ScannedModule> = discovered
            .into_iter()
            .map(|d| {
                ScannedModule::new(
                    d.module_id,
                    d.description,
                    d.input_schema,
                    d.output_schema,
                    d.tags,
                    d.target,
                )
            })
            .collect();

        writer.write(&scanned, &mut registry, false, false);
        Ok(())
    }

    /// Scan Axum routes and return module definitions.
    ///
    /// Uses `settings.scanner_source`. For "openapi", the document is read
    /// from `settings.openapi_spec` (`APCORE_OPENAPI_SPEC`) on every call;
    /// without it, this fails with a Scanner error.
    pub async fn scan(
        &self,
        router: &axum::Router,
        include: Option<&str>,
        exclude: Option<&str>,
    ) -> Result<Vec<ScannedModule>, AxumApcoreError> {
        let scanner = get_scanner_with_spec_path(
            &self.settings.scanner_source,
            self.settings.openapi_spec.as_deref(),
        )?;
        scanner.scan(router, include, exclude).await
    }

    /// Register scanned modules into the registry and executor.
    ///
    /// Modules are written to both the query registry (for `list_modules`)
    /// and the executor's internal registry (for `call`/`stream`).
    pub async fn register_modules(&self, modules: &[ScannedModule]) -> Result<(), AxumApcoreError> {
        let validator = AxumModuleValidator::new();
        for module in modules {
            let errors = validator.validate(&module.module_id);
            if !errors.is_empty() {
                tracing::warn!(
                    module_id = %module.module_id,
                    errors = ?errors,
                    "Module validation warnings"
                );
            }
        }

        // Build the writer and determine which modules have real handlers.
        let (writer, handler_targets) = {
            let handlers = self.handler_map.lock().expect("handler map lock poisoned");
            let targets: std::collections::HashSet<String> = handlers.keys().cloned().collect();
            let w = if handlers.is_empty() {
                AxumRegistryWriter::new()
            } else {
                AxumRegistryWriter::with_handler_map(handlers.clone())
            };
            (w, targets)
        };

        // Write all modules to the query registry (for list_modules, exports).
        {
            let mut registry = self.registry.lock().expect("registry lock poisoned");
            writer.write(modules, &mut registry, false, true);
        }

        // Write only modules with registered handlers to the executor's
        // registry. This avoids overwriting real handlers with passthrough
        // handlers when multiple AxumApcore instances share a global executor.
        //
        // The executor's `registry` is an interior-mutable `Arc<Registry>`, so
        // we register through `&self`. We register directly (rather than via the
        // toolkit `RegistryWriter`) for two reasons: we reuse the descriptor the
        // writer just produced for the query registry to keep both registries in
        // lockstep, and we need upsert semantics — `unregister` then `register`
        // — which the writer does not provide (apcore rejects duplicate IDs).
        let handler_modules: Vec<&ScannedModule> = modules
            .iter()
            .filter(|m| handler_targets.contains(&m.target))
            .collect();

        if !handler_modules.is_empty() {
            let handlers = self.handler_map.lock().expect("handler map lock poisoned");
            let registry = self.registry.lock().expect("registry lock poisoned");

            for module in &handler_modules {
                let Some(handler) = handlers.get(&module.target).cloned() else {
                    continue;
                };
                let descriptor = match registry.get_definition(&module.module_id) {
                    Ok(Some(d)) => d,
                    _ => continue,
                };
                let module_obj = apcore::decorator::FunctionModule::new::<_, ()>(
                    descriptor.annotations.clone().unwrap_or_default(),
                    descriptor.input_schema.clone(),
                    descriptor.output_schema.clone(),
                    move |inputs: Value, ctx: &Context<Value>| handler(inputs, ctx),
                );
                let _ = self.executor.registry.unregister(&module.module_id);
                let _ = self.executor.registry.register(
                    &module.module_id,
                    Box::new(module_obj),
                    descriptor,
                );
            }
        }

        Ok(())
    }

    /// Register a handler function for a target string.
    pub fn register_handler(&self, target: &str, handler: HandlerFn) {
        let mut handlers = self.handler_map.lock().expect("handler map lock poisoned");
        handlers.insert(target.to_string(), handler);
    }

    // ---- Execution ----

    /// Execute a module by ID.
    pub async fn call(
        &self,
        module_id: &str,
        inputs: Value,
        context: Option<&Context<Value>>,
    ) -> Result<Value, AxumApcoreError> {
        let result = self.executor.call(module_id, inputs, context, None).await?;
        Ok(result)
    }

    /// Execute a module with a default anonymous context.
    pub async fn call_anonymous(
        &self,
        module_id: &str,
        inputs: Value,
    ) -> Result<Value, AxumApcoreError> {
        let ctx = Context::new(anonymous_identity());
        self.call(module_id, inputs, Some(&ctx)).await
    }

    /// Execute a module with streaming output, collecting all chunks.
    ///
    /// apcore 0.22+ `Executor::stream` returns a `Stream` of result chunks
    /// (true streaming) rather than a future resolving to a `Vec`. This
    /// helper drains the stream into a `Vec`, short-circuiting on the first
    /// chunk error. Callers that need incremental delivery should consume the
    /// executor's stream directly.
    pub async fn stream(
        &self,
        module_id: &str,
        inputs: Value,
        context: Option<&Context<Value>>,
    ) -> Result<Vec<Value>, AxumApcoreError> {
        use futures::TryStreamExt;
        let results: Vec<Value> = self
            .executor
            .stream(module_id, inputs, context, None)
            .try_collect()
            .await?;
        Ok(results)
    }

    /// Execute a module with a timeout and cancellation support.
    ///
    /// The timeout covers both lock acquisition and execution. If the
    /// timeout elapses, the cancel token is triggered and a `ModuleTimeout`
    /// error is returned.
    pub async fn cancellable_call(
        &self,
        module_id: &str,
        inputs: Value,
        context: Option<&Context<Value>>,
        timeout: Duration,
    ) -> Result<Value, AxumApcoreError> {
        let cancel_token = CancelToken::new();

        // Build a context with the cancel token attached
        let ctx = match context {
            Some(parent) => {
                let mut child = parent.clone();
                child.cancel_token = Some(cancel_token.clone());
                child
            }
            None => {
                let mut ctx = Context::new(anonymous_identity());
                ctx.cancel_token = Some(cancel_token.clone());
                ctx
            }
        };

        let call_fut = self.executor.call(module_id, inputs, Some(&ctx), None);

        match tokio::time::timeout(timeout, call_fut).await {
            Ok(result) => Ok(result?),
            Err(_elapsed) => {
                cancel_token.cancel();
                Err(AxumApcoreError::Execution(apcore::ModuleError::new(
                    apcore::ErrorCode::ModuleTimeout,
                    format!(
                        "Module '{}' timed out after {}ms",
                        module_id,
                        timeout.as_millis()
                    ),
                )))
            }
        }
    }

    // ---- Task Management ----

    /// Submit an async task for background execution.
    ///
    /// The task runs the specified module in the background. Returns the task ID.
    pub fn submit_task(&self, module_id: &str, inputs: Value) -> Result<String, AxumApcoreError> {
        let task_id = uuid::Uuid::new_v4().to_string();
        let (task_id, cancel_token) = self.task_manager.submit(task_id, module_id.to_string())?;

        let executor = self.executor.clone();
        let task_manager = self.task_manager.clone();
        let module_id_owned = module_id.to_string();
        let task_id_clone = task_id.clone();

        tokio::spawn(async move {
            let mut ctx = Context::new(anonymous_identity());
            ctx.cancel_token = Some(cancel_token);

            let result = executor
                .call(&module_id_owned, inputs, Some(&ctx), None)
                .await;

            match result {
                Ok(value) => task_manager.complete(&task_id_clone, value),
                Err(e) => task_manager.fail(&task_id_clone, e.message),
            }
        });

        Ok(task_id)
    }

    /// Get the status of an async task.
    pub fn get_task_status(&self, task_id: &str) -> Option<TaskInfo> {
        self.task_manager.get_task_info(task_id)
    }

    /// Get the result of a completed task.
    pub fn get_task_result(&self, task_id: &str) -> Option<Value> {
        self.task_manager.get_result(task_id)
    }

    /// Cancel a running async task.
    pub fn cancel_task(&self, task_id: &str) -> bool {
        self.task_manager.cancel(task_id)
    }

    /// List async tasks, optionally filtered by status.
    pub fn list_tasks(&self, status: Option<&str>) -> Vec<TaskInfo> {
        self.task_manager.list_tasks(status)
    }

    // ---- Export ----

    /// Export registered modules as OpenAI-compatible tool definitions.
    #[cfg(feature = "mcp")]
    pub fn to_openai_tools(
        &self,
        embed_annotations: bool,
        strict: bool,
        tags: Option<Vec<String>>,
        prefix: Option<String>,
    ) -> Result<Vec<Value>, AxumApcoreError> {
        let registry = self.registry.lock().expect("registry lock poisoned");
        let registry_arc = Arc::new(registry_snapshot(&registry));

        let config = apcore_mcp::OpenAIToolsConfig {
            embed_annotations,
            strict,
            tags,
            prefix,
        };
        apcore_mcp::to_openai_tools(apcore_mcp::BackendSource::Registry(registry_arc), config)
            .map_err(|e| AxumApcoreError::Config(format!("OpenAI export failed: {e}")))
    }

    // ---- MCP Server ----

    /// Create an MCP server backed by the live executor (requires "mcp" feature).
    ///
    /// The server is wired to the same `Arc<Executor>` this client executes
    /// against (apcore-mcp 0.17 actually drives the transport and registers the
    /// executor's modules as tools), so MCP tool calls run the real registered
    /// handlers rather than serving schema-only definitions.
    #[cfg(feature = "mcp")]
    pub fn create_mcp_server(&self) -> Result<apcore_mcp::MCPServer, AxumApcoreError> {
        let transport: apcore_mcp::TransportKind = self
            .settings
            .serve_transport
            .parse()
            .map_err(|e| AxumApcoreError::Config(format!("Invalid transport: {e}")))?;

        let config = apcore_mcp::MCPServerConfig {
            transport,
            host: self.settings.serve_host.clone(),
            port: self.settings.serve_port,
            name: self.settings.server_name.clone(),
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
            validate_inputs: true,
            require_auth: self.settings.jwt_secret.is_some(),
            trace: self.settings.tracing,
            explorer: self.settings.explorer_enabled,
            explorer_prefix: self.settings.explorer_prefix.clone(),
            ..Default::default()
        };

        let backend = apcore_mcp::RegistryOrExecutor::Executor(self.executor.clone());
        Ok(apcore_mcp::MCPServer::with_registry_or_executor(
            backend, config,
        ))
    }

    // ---- Accessors ----

    pub fn settings(&self) -> &ApcoreSettings {
        &self.settings
    }

    pub fn registry(&self) -> Arc<Mutex<Registry>> {
        self.registry.clone()
    }

    pub fn executor(&self) -> Arc<Executor> {
        self.executor.clone()
    }

    pub fn context_factory(&self) -> Arc<AxumContextFactory> {
        self.context_factory.clone()
    }

    pub fn task_manager(&self) -> &TaskManager {
        &self.task_manager
    }

    pub fn observability(&self) -> &ObservabilityState {
        &self.observability
    }

    /// List registered module IDs.
    pub fn list_modules(&self) -> Vec<String> {
        let registry = self.registry.lock().expect("registry lock poisoned");
        registry.list(None, None, None)
    }

    // ---- CLI (HTTP proxy) ----

    /// Create an apcore-cli clap Command with all scanned routes as CLI commands.
    ///
    /// Scans Axum routes, registers them as HTTP proxy modules, and returns
    /// a clap Command ready for dispatch. Each CLI command forwards requests
    /// to the running REST API via HTTP.
    ///
    /// Requires the `cli` feature.
    ///
    /// # Arguments
    ///
    /// * `router` — The Axum Router to scan.
    /// * `config` — Configuration for the CLI builder.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use axum_apcore::{AxumApcore, CreateCliConfig};
    ///
    /// let apcore = AxumApcore::new();
    /// let config = CreateCliConfig {
    ///     base_url: "http://localhost:3000".into(),
    ///     ..Default::default()
    /// };
    /// let cmd = apcore.create_cli(&router, config).await?;
    /// ```
    #[cfg(feature = "cli")]
    pub async fn create_cli(
        &self,
        router: &axum::Router,
        mut config: CreateCliConfig,
    ) -> Result<clap::Command, AxumApcoreError> {
        use apcore_cli::discovery::{
            register_describe_command, register_list_command, RegistryProvider,
        };
        use apcore_cli::shell::{register_completion_command, register_man_command};
        use apcore_cli::{
            build_module_command_with_limit, register_init_command, set_all_options_help,
            set_docs_url, ApCoreRegistryProvider,
        };
        use apcore_toolkit::HTTPProxyRegistryWriter;

        // Apply apcore-cli global settings. `set_verbose_help` was renamed to
        // `set_all_options_help` in apcore-cli 0.9.
        set_all_options_help(config.verbose_help);
        set_docs_url(config.docs_url.clone());

        // 1. Scan routes using the config's scanner source (not self.settings);
        // the "openapi" source scans `config.openapi_spec`.
        let scanner =
            crate::scanner::get_scanner_with_spec(&config.scan_source, config.openapi_spec.take())?;
        let modules = scanner
            .scan(router, config.include.as_deref(), config.exclude.as_deref())
            .await?;
        tracing::info!(
            count = modules.len(),
            "Scanned {} API routes for CLI",
            modules.len()
        );

        // 2. Register as HTTP proxy modules. The writer constructor is now
        // fallible (apcore-toolkit 0.8 validates the base URL and timeout), and
        // its `write` takes `&Registry` (interior-mutable) since toolkit 0.9.
        let proxy_registry = Registry::new();
        let writer = HTTPProxyRegistryWriter::new(
            config.base_url.clone(),
            config.auth_header_factory,
            config.timeout,
        )
        .map_err(|e| AxumApcoreError::Config(format!("Invalid HTTP proxy config: {e}")))?;
        let results = writer.write(&modules, &proxy_registry);
        let registered = results.iter().filter(|r| r.verified).count();
        tracing::info!(
            registered,
            total = results.len(),
            "Registered HTTP proxy modules"
        );

        // 3. Wrap the proxy registry in a RegistryProvider used both for
        // building per-module commands and for discovery-command dispatch.
        let provider = ApCoreRegistryProvider::new(proxy_registry);

        // 4. Build the root clap Command.
        let cli_description = format!(
            "{prog} — CLI for Axum API.\n\n\
             Tips:\n\
             \x20 {prog} list                List all available commands\n\
             \x20 {prog} describe MODULE_ID  Show schema and annotations for a command",
            prog = config.prog_name,
        );

        let mut cmd = clap::Command::new(config.prog_name.clone())
            .about(cli_description)
            .version(env!("CARGO_PKG_VERSION"))
            .subcommand_required(true)
            .arg_required_else_help(true);

        // 5. One subcommand per scanned route. apcore-cli 0.7 removed
        // `GroupedModuleGroup`; `build_module_command_with_limit` constructs a
        // clap Command directly from each module descriptor.
        for module_id in RegistryProvider::list(&provider) {
            let Some(descriptor) = provider.get_module_descriptor(&module_id) else {
                continue;
            };
            match build_module_command_with_limit(&descriptor, config.help_text_max_length) {
                Ok(sub) => cmd = cmd.subcommand(sub),
                Err(e) => {
                    tracing::warn!(module_id = %module_id, error = %e, "Skipping module command")
                }
            }
        }

        // 6. Built-in discovery, shell, and init subcommands. apcore-cli 0.9
        // removed the `register_discovery_commands` / `register_shell_commands`
        // umbrella helpers in favor of per-command registrars.
        cmd = register_list_command(cmd);
        cmd = register_describe_command(cmd);
        cmd = register_completion_command(cmd);
        cmd = register_man_command(cmd);
        cmd = register_init_command(cmd);

        Ok(cmd)
    }
}

impl Default for AxumApcore {
    fn default() -> Self {
        Self::new()
    }
}

/// Configuration for [`AxumApcore::create_cli`].
#[cfg(feature = "cli")]
pub struct CreateCliConfig {
    /// CLI program name shown in help text.
    pub prog_name: String,
    /// Base URL of the running Axum API server.
    pub base_url: String,
    /// Optional callable returning HTTP auth headers.
    pub auth_header_factory: Option<Box<dyn Fn() -> HashMap<String, String> + Send + Sync>>,
    /// HTTP request timeout in seconds.
    pub timeout: f64,
    /// Scanner source: "native" or "openapi" (requires the `openapi` feature
    /// and `openapi_spec`).
    pub scan_source: String,
    /// OpenAPI 3.x document scanned when `scan_source` is "openapi", e.g.
    /// `serde_json::to_value(ApiDoc::openapi())` for a utoipa app, or a file
    /// read with `axum_apcore::scanner::openapi::load_spec_file`. Ignored by
    /// other sources.
    pub openapi_spec: Option<Value>,
    /// Include regex filter for module IDs.
    pub include: Option<String>,
    /// Exclude regex filter for module IDs.
    pub exclude: Option<String>,
    /// Max characters for CLI help text per command.
    pub help_text_max_length: usize,
    /// Base URL for online documentation.
    pub docs_url: Option<String>,
    /// Show built-in apcore options in help output.
    pub verbose_help: bool,
}

#[cfg(feature = "cli")]
impl Default for CreateCliConfig {
    fn default() -> Self {
        Self {
            prog_name: "axum-apcore-cli".to_string(),
            base_url: "http://localhost:3000".to_string(),
            auth_header_factory: None,
            timeout: 60.0,
            scan_source: "native".to_string(),
            openapi_spec: None,
            include: None,
            exclude: None,
            help_text_max_length: 1000,
            docs_url: None,
            verbose_help: false,
        }
    }
}

/// Create an anonymous identity for default contexts.
fn anonymous_identity() -> Identity {
    // apcore 0.16 made Identity fields private; use the canonical constructor.
    Identity::new(
        "anonymous".into(),
        "anonymous".into(),
        vec![],
        HashMap::new(),
    )
}

/// Create a snapshot copy of a registry (schema-only, no handlers).
#[cfg(feature = "mcp")]
fn registry_snapshot(source: &Registry) -> Registry {
    // We create a new empty registry for the backend source.
    // The MCPServer will read module descriptors from the registry it's given.
    // Since we hold a lock, we copy descriptor data here.
    // `Registry::register` is interior-mutable (`&self`) in apcore 0.24, so
    // the target registry does not need to be `mut`.
    let target = Registry::new();
    for name in source.list(None, None, None) {
        // apcore 0.24 changed `get_definition` to return a Result; treat any
        // error or missing definition as "skip this module".
        if let Ok(Some(descriptor)) = source.get_definition(&name) {
            // Register with passthrough handler — MCP only needs the schema.
            // `annotations` is now `Option<ModuleAnnotations>` (apcore 0.18.1).
            let fm = apcore::decorator::FunctionModule::new::<_, ()>(
                descriptor.annotations.clone().unwrap_or_default(),
                descriptor.input_schema.clone(),
                descriptor.output_schema.clone(),
                |inputs: Value,
                 _ctx: &Context<Value>|
                 -> std::pin::Pin<
                    Box<
                        dyn std::future::Future<Output = Result<Value, apcore::ModuleError>>
                            + Send
                            + '_,
                    >,
                > { Box::pin(async move { Ok(inputs) }) },
            );
            // Ignore registration errors (e.g., duplicate names in edge cases)
            let _ = target.register(&name, Box::new(fm), descriptor.clone());
        }
    }
    target
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_creates_instance() {
        let apcore = AxumApcore::new();
        assert_eq!(apcore.settings().serve_port, 9090);
    }

    #[test]
    fn test_with_settings() {
        let settings = ApcoreSettings {
            serve_port: 8080,
            ..ApcoreSettings::default()
        };
        let apcore = AxumApcore::with_settings(settings);
        assert_eq!(apcore.settings().serve_port, 8080);
    }

    #[test]
    fn test_list_modules_empty() {
        let apcore = AxumApcore::new();
        let _ = apcore.list_modules();
    }

    #[test]
    fn test_anonymous_identity() {
        let id = anonymous_identity();
        assert_eq!(id.id(), "anonymous");
        assert_eq!(id.identity_type(), "anonymous");
        assert!(id.roles().is_empty());
    }

    #[test]
    fn test_submit_and_list_tasks() {
        let apcore = AxumApcore::new();
        let tasks = apcore.list_tasks(None);
        // May contain tasks from other tests due to shared state, but should not panic
        let _ = tasks;
    }
}

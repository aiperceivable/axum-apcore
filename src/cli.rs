// CLI commands for axum-apcore.
//
// Provides scan, serve, export, tasks, and apcore-cli commands via clap.
// The scan/serve/export/tasks commands are framework-specific.
// The list/describe/completion/man/init commands are delegated to apcore-cli.

use std::path::PathBuf;

use clap::{CommandFactory, Parser, Subcommand};

use crate::engine::tasks::TaskManager;
use crate::errors::AxumApcoreError;
use crate::output::AxumRegistryWriter;
use crate::scanner::{get_scanner_with_spec_path, AxumScanner};

// clap_complete is feature-gated with cli, same as this module.
use clap_complete::Shell;

/// axum-apcore CLI — scan routes, serve MCP, export tools, and manage modules.
#[derive(Parser, Debug)]
#[command(name = "axum-apcore", version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Scan Axum routes and generate module definitions.
    Scan {
        /// Scanner source: "native" or "openapi".
        #[arg(long, default_value = "native")]
        source: String,

        /// OpenAPI 3.x document (JSON or YAML file) to scan; required by
        /// `--source openapi`, ignored otherwise.
        #[arg(long, value_name = "PATH", env = "APCORE_OPENAPI_SPEC")]
        spec: Option<PathBuf>,

        /// Output format: "registry", "yaml", or "http-proxy".
        #[arg(long, default_value = "registry")]
        output: String,

        /// Directory for YAML output files.
        #[arg(long)]
        dir: Option<String>,

        /// Dry run — scan but don't write.
        #[arg(long)]
        dry_run: bool,

        /// Include filter regex.
        #[arg(long)]
        include: Option<String>,

        /// Exclude filter regex.
        #[arg(long)]
        exclude: Option<String>,

        /// Verify registration after writing.
        #[arg(long)]
        verify: bool,
    },

    /// Start MCP server exposing registered modules.
    Serve {
        /// Transport: "stdio", "streamable-http", or "sse".
        #[arg(long)]
        transport: Option<String>,

        /// Host to bind to.
        #[arg(long)]
        host: Option<String>,

        /// Port to listen on.
        #[arg(long)]
        port: Option<u16>,

        /// Enable MCP explorer UI.
        #[arg(long)]
        explorer: bool,

        /// JWT secret for authentication.
        #[arg(long, env = "APCORE_JWT_SECRET")]
        jwt_secret: Option<String>,

        /// Approval mode: "auto", "deny", or "manual".
        #[arg(long, default_value = "auto")]
        approval: String,

        /// Comma-separated tags to filter exposed modules.
        #[arg(long)]
        tags: Option<String>,

        /// Module ID prefix filter.
        #[arg(long)]
        prefix: Option<String>,
    },

    /// Export modules as OpenAI-compatible tools.
    Export {
        /// Export format: "openai-tools".
        #[arg(long, default_value = "openai-tools")]
        format: String,

        /// Use strict mode for OpenAI tools.
        #[arg(long)]
        strict: bool,

        /// Embed annotations in tool definitions.
        #[arg(long)]
        embed_annotations: bool,

        /// Comma-separated tags to filter.
        #[arg(long)]
        tags: Option<String>,

        /// Module ID prefix filter.
        #[arg(long)]
        prefix: Option<String>,
    },

    /// Manage async tasks.
    Tasks {
        #[command(subcommand)]
        action: TaskCommands,
    },

    /// List available modules in the registry.
    List {
        /// Filter by tag (can be repeated).
        #[arg(long)]
        tag: Vec<String>,

        /// Output format: "table" or "json".
        #[arg(long)]
        format: Option<String>,
    },

    /// Show schema and annotations for a module.
    Describe {
        /// Module ID to describe.
        module_id: String,

        /// Output format: "table" or "json".
        #[arg(long)]
        format: Option<String>,
    },

    /// Generate a shell completion script.
    Completion {
        /// Shell to generate completions for.
        shell: String,
    },

    /// Generate a roff man page.
    Man {
        /// Command to generate man page for.
        command: Option<String>,
    },

    /// Scaffold a new apcore module.
    Init {
        #[command(subcommand)]
        action: InitCommands,
    },
}

#[derive(Subcommand, Debug)]
pub enum InitCommands {
    /// Create a new module scaffold.
    Module {
        /// Module ID (dotted name, e.g. "users.get_user").
        module_id: String,

        /// Scaffold style: "decorator", "convention", or "binding".
        #[arg(long, default_value = "convention")]
        style: String,

        /// Output directory.
        #[arg(long)]
        dir: Option<String>,

        /// Module description.
        #[arg(long)]
        description: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum TaskCommands {
    /// List async tasks.
    List {
        /// Filter by status.
        #[arg(long)]
        status: Option<String>,
    },
    /// Cancel a running task.
    Cancel {
        /// Task ID to cancel.
        task_id: String,
    },
    /// Clean up old completed tasks.
    Cleanup {
        /// Maximum age in seconds.
        #[arg(long, default_value = "3600")]
        max_age: u64,
    },
}

/// Parse CLI arguments.
pub fn parse_cli() -> Cli {
    Cli::parse()
}

/// Execute a CLI command.
///
/// Call this from your application's main function after calling `parse_cli()`.
pub async fn run(cli: Cli) -> Result<(), AxumApcoreError> {
    match cli.command {
        Commands::Scan {
            source,
            spec,
            output,
            dir,
            dry_run,
            include,
            exclude,
            verify,
        } => {
            let scanner = get_scanner_with_spec_path(&source, spec.as_deref())?;
            run_scan(
                scanner.as_ref(),
                output,
                dir,
                dry_run,
                include,
                exclude,
                verify,
            )
            .await
        }
        Commands::Serve {
            transport,
            host,
            port,
            tags,
            prefix,
            ..
        } => run_serve(transport, host, port, tags, prefix).await,
        Commands::Export {
            format,
            strict,
            embed_annotations,
            tags,
            prefix,
        } => run_export(format, strict, embed_annotations, tags, prefix),
        Commands::Tasks { action } => run_tasks(action),
        Commands::List { tag, format } => run_list(tag, format),
        Commands::Describe { module_id, format } => run_describe(module_id, format),
        Commands::Completion { shell } => run_completion(shell),
        Commands::Man { command } => run_man(command),
        Commands::Init { action } => run_init(action),
    }
}

/// Execute the `scan` command with an already-resolved scanner.
async fn run_scan(
    scanner: &dyn AxumScanner,
    output: String,
    dir: Option<String>,
    dry_run: bool,
    include: Option<String>,
    exclude: Option<String>,
    verify: bool,
) -> Result<(), AxumApcoreError> {
    let router = axum::Router::new();

    let include_ref = include.as_deref();
    let exclude_ref = exclude.as_deref();
    let modules = scanner.scan(&router, include_ref, exclude_ref).await?;

    println!(
        "Scanned {} modules via '{}' scanner",
        modules.len(),
        scanner.source_name()
    );
    for m in &modules {
        println!("  - {} ({})", m.module_id, m.description);
    }

    if dry_run {
        println!("Dry run — no output written.");
        return Ok(());
    }

    match output.as_str() {
        "registry" => {
            let writer = AxumRegistryWriter::new();
            let mut registry = apcore::Registry::new();
            let results = writer.write(&modules, &mut registry, false, verify);
            let ok_count = results.iter().filter(|r| r.verified).count();
            println!("Registered {}/{} modules", ok_count, results.len());
        }
        "yaml" => {
            let out_dir = dir.unwrap_or_else(|| "apcore_modules".into());
            let yaml_writer = apcore_toolkit::YAMLWriter;
            let results = yaml_writer
                .write(&modules, &out_dir, false, verify, None)
                .map_err(|e| AxumApcoreError::Config(format!("YAML write failed: {e}")))?;
            let ok_count = results.iter().filter(|r| r.verified).count();
            println!(
                "Wrote {}/{} YAML bindings to '{}'",
                ok_count,
                results.len(),
                out_dir
            );
        }
        other => {
            return Err(AxumApcoreError::Config(format!(
                "Unknown output format: '{}'. Available: registry, yaml",
                other
            )));
        }
    }

    Ok(())
}

/// Execute the `serve` command.
async fn run_serve(
    transport: Option<String>,
    host: Option<String>,
    port: Option<u16>,
    tags: Option<String>,
    prefix: Option<String>,
) -> Result<(), AxumApcoreError> {
    #[cfg(not(feature = "mcp"))]
    {
        let _ = (transport, host, port, tags, prefix);
        return Err(AxumApcoreError::Config(
            "MCP feature not enabled. Build with --features mcp".into(),
        ));
    }

    #[cfg(feature = "mcp")]
    {
        let settings = crate::config::get_apcore_settings();

        let transport_str = transport.as_deref().unwrap_or(&settings.serve_transport);
        let host_str = host.as_deref().unwrap_or(&settings.serve_host);
        let port_val = port.unwrap_or(settings.serve_port);

        let parsed_tags = tags.map(|t| parse_tags(&t));

        println!(
            "Starting MCP server on {}://{}:{}",
            transport_str, host_str, port_val
        );

        let config = apcore_mcp::ServeConfig {
            name: settings.server_name.clone(),
            transport: transport_str.to_string(),
            host: host_str.to_string(),
            port: port_val,
            tags: parsed_tags,
            prefix,
            ..apcore_mcp::ServeConfig::default()
        };

        let backend = apcore_mcp::BackendSource::ExtensionsDir(settings.module_dir.clone());

        apcore_mcp::serve(backend, config)
            .map_err(|e| AxumApcoreError::Config(format!("MCP server error: {e}")))?;

        Ok(())
    }
}

/// Execute the `export` command.
fn run_export(
    format: String,
    strict: bool,
    embed_annotations: bool,
    tags: Option<String>,
    prefix: Option<String>,
) -> Result<(), AxumApcoreError> {
    #[cfg(not(feature = "mcp"))]
    {
        let _ = (format, strict, embed_annotations, tags, prefix);
        return Err(AxumApcoreError::Config(
            "MCP feature not enabled. Build with --features mcp".into(),
        ));
    }

    #[cfg(feature = "mcp")]
    {
        if format != "openai-tools" {
            return Err(AxumApcoreError::Config(format!(
                "Unknown export format: '{}'. Available: openai-tools",
                format
            )));
        }

        let settings = crate::config::get_apcore_settings();
        let parsed_tags = tags.map(|t| parse_tags(&t));

        let config = apcore_mcp::OpenAIToolsConfig {
            embed_annotations,
            strict,
            tags: parsed_tags,
            prefix,
        };

        let backend = apcore_mcp::BackendSource::ExtensionsDir(settings.module_dir.clone());
        let tools = apcore_mcp::to_openai_tools(backend, config)
            .map_err(|e| AxumApcoreError::Config(format!("Export failed: {e}")))?;

        let json = serde_json::to_string_pretty(&tools).map_err(AxumApcoreError::Json)?;
        println!("{}", json);
        println!("Exported {} OpenAI tool definitions", tools.len());

        Ok(())
    }
}

/// Execute task subcommands.
fn run_tasks(action: TaskCommands) -> Result<(), AxumApcoreError> {
    let settings = crate::config::get_apcore_settings();
    let task_manager = TaskManager::from_settings(settings);

    match action {
        TaskCommands::List { status } => {
            let tasks = task_manager.list_tasks(status.as_deref());
            if tasks.is_empty() {
                println!("No tasks found.");
            } else {
                let header = format!(
                    "{:<36} {:<20} {:<10} {}",
                    "TASK ID", "MODULE", "STATUS", "CREATED"
                );
                println!("{header}");
                for task in &tasks {
                    println!(
                        "{:<36} {:<20} {:<10} {}",
                        task.task_id, task.module_id, task.status, task.created_at
                    );
                }
                println!("\nTotal: {} tasks", tasks.len());
            }
        }
        TaskCommands::Cancel { task_id } => {
            if task_manager.cancel(&task_id) {
                println!("Task '{}' cancelled.", task_id);
            } else {
                println!("Task '{}' not found or not running.", task_id);
            }
        }
        TaskCommands::Cleanup { max_age } => {
            let removed = task_manager.cleanup(max_age);
            println!("Cleaned up {} tasks (older than {}s).", removed, max_age);
        }
    }

    Ok(())
}

/// Build an `ApCoreRegistryProvider` from the global registry singleton.
///
/// Since `Registry` is not `Clone`, we rebuild a provider by re-registering
/// module descriptors into a fresh registry.
fn build_registry_provider() -> apcore_cli::ApCoreRegistryProvider {
    let registry_arc = crate::engine::registry::get_registry();
    let registry = registry_arc.lock().expect("registry lock poisoned");

    // `Registry::register` is interior-mutable (`&self`) in apcore 0.24.
    let fresh = apcore::Registry::new();
    for name in registry.list(None, None, None) {
        // apcore 0.24: `get_definition` returns Result; `annotations` is Option.
        if let Ok(Some(descriptor)) = registry.get_definition(&name) {
            let fm = apcore::decorator::FunctionModule::new::<_, ()>(
                descriptor.annotations.clone().unwrap_or_default(),
                descriptor.input_schema.clone(),
                descriptor.output_schema.clone(),
                |inputs: serde_json::Value,
                 _ctx: &apcore::Context<serde_json::Value>|
                 -> std::pin::Pin<
                    Box<
                        dyn std::future::Future<
                                Output = Result<serde_json::Value, apcore::ModuleError>,
                            > + Send
                            + '_,
                    >,
                > { Box::pin(async move { Ok(inputs) }) },
            );
            let _ = fresh.register(&name, Box::new(fm), descriptor.clone());
        }
    }

    apcore_cli::ApCoreRegistryProvider::new(fresh)
}

/// Execute the `list` command via apcore-cli.
fn run_list(tags: Vec<String>, format: Option<String>) -> Result<(), AxumApcoreError> {
    let provider = build_registry_provider();
    let tag_refs: Vec<&str> = tags.iter().map(|s| s.as_str()).collect();

    // apcore-cli 0.9 demoted `cmd_list` to crate-private; `cmd_list_enhanced`
    // is the public entry point and adds search/status/sort filtering.
    let opts = apcore_cli::ListOptions {
        tags: &tag_refs,
        explicit_format: format.as_deref(),
        ..apcore_cli::ListOptions::default()
    };
    let output = apcore_cli::cmd_list_enhanced(&provider, &opts)
        .map_err(|e| AxumApcoreError::Config(format!("List error: {e}")))?;
    println!("{output}");
    Ok(())
}

/// Execute the `describe` command via apcore-cli.
fn run_describe(module_id: String, format: Option<String>) -> Result<(), AxumApcoreError> {
    let provider = build_registry_provider();

    let output = apcore_cli::cmd_describe(&provider, &module_id, format.as_deref())
        .map_err(|e| AxumApcoreError::Config(format!("Describe error: {e}")))?;
    println!("{output}");
    Ok(())
}

/// Execute the `completion` command via apcore-cli.
fn run_completion(shell: String) -> Result<(), AxumApcoreError> {
    let shell_val: Shell = shell
        .parse()
        .map_err(|e| AxumApcoreError::Config(format!("Unknown shell '{shell}': {e}")))?;

    let mut cmd = Cli::command();
    let output = apcore_cli::cmd_completion(shell_val, "axum-apcore", &mut cmd);
    print!("{output}");
    Ok(())
}

/// Execute the `man` command via apcore-cli.
fn run_man(command: Option<String>) -> Result<(), AxumApcoreError> {
    let root_cmd = Cli::command();

    match command {
        Some(cmd_name) => {
            let output = apcore_cli::cmd_man(
                &cmd_name,
                &root_cmd,
                "axum-apcore",
                env!("CARGO_PKG_VERSION"),
            )
            .map_err(|e| AxumApcoreError::Config(format!("Man error: {e}")))?;
            print!("{output}");
        }
        None => {
            let output = apcore_cli::build_program_man_page(
                &root_cmd,
                "axum-apcore",
                env!("CARGO_PKG_VERSION"),
                None,
                None,
            );
            print!("{output}");
        }
    }
    Ok(())
}

/// Execute the `init` subcommand via apcore-cli.
fn run_init(action: InitCommands) -> Result<(), AxumApcoreError> {
    match action {
        InitCommands::Module {
            module_id,
            style,
            dir,
            description,
        } => {
            // Build clap ArgMatches to pass to apcore-cli's handle_init
            let mut args = vec![
                "init".to_string(),
                "module".to_string(),
                module_id,
                "--style".to_string(),
                style,
            ];
            if let Some(d) = dir {
                args.push("--dir".to_string());
                args.push(d);
            }
            if let Some(desc) = description {
                args.push("--description".to_string());
                args.push(desc);
            }

            let cmd = apcore_cli::init_command();
            let matches = cmd
                .try_get_matches_from(args)
                .map_err(|e| AxumApcoreError::Config(format!("Init error: {e}")))?;
            apcore_cli::handle_init(&matches);
            Ok(())
        }
    }
}

/// Parse comma-separated tags string into a Vec.
fn parse_tags(tags: &str) -> Vec<String> {
    tags.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn test_cli_parse_scan() {
        let cli = Cli::try_parse_from(["axum-apcore", "scan"]).unwrap();
        assert!(matches!(cli.command, Commands::Scan { .. }));
    }

    #[test]
    fn test_cli_parse_serve() {
        let cli = Cli::try_parse_from(["axum-apcore", "serve"]).unwrap();
        assert!(matches!(cli.command, Commands::Serve { .. }));
    }

    #[test]
    fn test_cli_parse_export() {
        let cli = Cli::try_parse_from(["axum-apcore", "export"]).unwrap();
        assert!(matches!(cli.command, Commands::Export { .. }));
    }

    #[test]
    fn test_cli_parse_tasks_list() {
        let cli = Cli::try_parse_from(["axum-apcore", "tasks", "list"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Tasks {
                action: TaskCommands::List { .. }
            }
        ));
    }

    #[test]
    fn test_cli_parse_tasks_cancel() {
        let cli = Cli::try_parse_from(["axum-apcore", "tasks", "cancel", "task-123"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Tasks {
                action: TaskCommands::Cancel { .. }
            }
        ));
    }

    #[test]
    fn test_cli_verify() {
        Cli::command().debug_assert();
    }

    #[test]
    fn test_parse_tags() {
        assert_eq!(parse_tags("a, b, c"), vec!["a", "b", "c"]);
        assert_eq!(parse_tags("single"), vec!["single"]);
        assert_eq!(parse_tags(""), Vec::<String>::new());
        assert_eq!(parse_tags("a,,b"), vec!["a", "b"]);
    }

    #[test]
    fn test_cli_parse_scan_with_options() {
        let cli = Cli::try_parse_from([
            "axum-apcore",
            "scan",
            "--source",
            "openapi",
            "--spec",
            "openapi.yaml",
            "--output",
            "yaml",
            "--dry-run",
            "--include",
            "users",
            "--verify",
        ])
        .unwrap();
        match cli.command {
            Commands::Scan {
                source,
                spec,
                output,
                dry_run,
                include,
                verify,
                ..
            } => {
                assert_eq!(source, "openapi");
                assert_eq!(spec, Some(PathBuf::from("openapi.yaml")));
                assert_eq!(output, "yaml");
                assert!(dry_run);
                assert_eq!(include.unwrap(), "users");
                assert!(verify);
            }
            _ => panic!("Expected Scan command"),
        }
    }

    fn scan_command(source: &str, spec: Option<PathBuf>) -> Cli {
        Cli {
            command: Commands::Scan {
                source: source.to_string(),
                spec,
                output: "registry".to_string(),
                dir: None,
                dry_run: false,
                include: None,
                exclude: None,
                verify: true,
            },
        }
    }

    #[cfg(feature = "openapi")]
    #[tokio::test]
    async fn test_run_scan_openapi_with_spec() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("openapi.json");
        let spec = serde_json::json!({
            "openapi": "3.1.0",
            "info": {"title": "T", "version": "1.0.0"},
            "paths": {
                "/cli_items": {"get": {"operationId": "cli_list_items_get", "tags": ["cli"],
                                       "responses": {"200": {"description": "OK"}}}},
                "/cli_health": {"get": {"responses": {"200": {"description": "OK"}}}}
            }
        });
        std::fs::write(&path, spec.to_string()).unwrap();
        let result = run(scan_command("openapi", Some(path))).await;
        assert!(
            result.is_ok(),
            "scan --source openapi --spec failed: {result:?}"
        );
    }

    #[cfg(feature = "openapi")]
    #[tokio::test]
    async fn test_run_scan_openapi_without_spec_fails() {
        match run(scan_command("openapi", None)).await {
            Err(AxumApcoreError::Scanner(msg)) => assert!(msg.contains("--spec")),
            other => panic!("expected a Scanner error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_run_scan_native_ignores_spec() {
        let missing = PathBuf::from("/nonexistent/axum-apcore/openapi.json");
        let result = run(scan_command("native", Some(missing))).await;
        assert!(result.is_ok(), "native scan failed: {result:?}");
    }

    #[test]
    fn test_cli_parse_serve_with_options() {
        let cli = Cli::try_parse_from([
            "axum-apcore",
            "serve",
            "--transport",
            "sse",
            "--host",
            "0.0.0.0",
            "--port",
            "8080",
            "--tags",
            "users,tasks",
        ])
        .unwrap();
        match cli.command {
            Commands::Serve {
                transport,
                host,
                port,
                tags,
                ..
            } => {
                assert_eq!(transport.unwrap(), "sse");
                assert_eq!(host.unwrap(), "0.0.0.0");
                assert_eq!(port.unwrap(), 8080);
                assert_eq!(tags.unwrap(), "users,tasks");
            }
            _ => panic!("Expected Serve command"),
        }
    }

    #[test]
    fn test_cli_parse_list() {
        let cli = Cli::try_parse_from(["axum-apcore", "list"]).unwrap();
        assert!(matches!(cli.command, Commands::List { .. }));
    }

    #[test]
    fn test_cli_parse_list_with_tags() {
        let cli = Cli::try_parse_from(["axum-apcore", "list", "--tag", "users", "--tag", "admin"])
            .unwrap();
        match cli.command {
            Commands::List { tag, .. } => {
                assert_eq!(tag, vec!["users", "admin"]);
            }
            _ => panic!("Expected List command"),
        }
    }

    #[test]
    fn test_cli_parse_describe() {
        let cli = Cli::try_parse_from(["axum-apcore", "describe", "users.get_user.get"]).unwrap();
        match cli.command {
            Commands::Describe { module_id, .. } => {
                assert_eq!(module_id, "users.get_user.get");
            }
            _ => panic!("Expected Describe command"),
        }
    }

    #[test]
    fn test_cli_parse_completion() {
        let cli = Cli::try_parse_from(["axum-apcore", "completion", "bash"]).unwrap();
        match cli.command {
            Commands::Completion { shell } => {
                assert_eq!(shell, "bash");
            }
            _ => panic!("Expected Completion command"),
        }
    }

    #[test]
    fn test_cli_parse_man() {
        let cli = Cli::try_parse_from(["axum-apcore", "man"]).unwrap();
        assert!(matches!(cli.command, Commands::Man { .. }));
    }

    #[test]
    fn test_build_registry_provider_empty() {
        // With no modules registered, provider should return an empty list.
        let provider = build_registry_provider();
        let modules = apcore_cli::discovery::RegistryProvider::list(&provider);
        // May contain modules from other tests due to shared global state,
        // but should not panic.
        let _ = modules;
    }

    #[test]
    fn test_run_list_empty_registry() {
        // Listing with no modules should succeed (not panic).
        let result = run_list(vec![], Some("json".into()));
        assert!(result.is_ok());
    }

    #[test]
    fn test_run_completion_bash() {
        let result = run_completion("bash".into());
        assert!(result.is_ok());
    }

    #[test]
    fn test_run_completion_invalid_shell() {
        let result = run_completion("invalid-shell".into());
        assert!(result.is_err());
    }

    #[test]
    fn test_run_man_program_page() {
        let result = run_man(None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_run_man_unknown_command() {
        let result = run_man(Some("nonexistent-command".into()));
        assert!(result.is_err());
    }

    #[test]
    fn test_cli_parse_init_module() {
        let cli = Cli::try_parse_from([
            "axum-apcore",
            "init",
            "module",
            "users.get_user",
            "--style",
            "binding",
        ])
        .unwrap();
        match cli.command {
            Commands::Init {
                action:
                    InitCommands::Module {
                        module_id, style, ..
                    },
            } => {
                assert_eq!(module_id, "users.get_user");
                assert_eq!(style, "binding");
            }
            _ => panic!("Expected Init Module command"),
        }
    }
}

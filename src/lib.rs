// axum-apcore — Axum integration for the apcore AI-Perceivable Core ecosystem.
//
// Exposes Axum routes as apcore modules via automatic scanning,
// with full execution, context mapping, and MCP serving support.

#![allow(clippy::result_large_err)]

pub mod client;
pub mod config;
pub mod context;
pub mod engine;
pub mod errors;
pub mod output;
pub mod scanner;

#[cfg(feature = "cli")]
pub mod cli;

// Re-export primary types at crate root for convenience.
pub use client::AxumApcore;
pub use config::ApcoreSettings;
pub use context::{ApContext, AxumContextFactory, RequestIdentity};
pub use engine::extensions::{AxumDiscoverer, AxumModuleValidator};
pub use engine::registry::{get_executor, get_registry};
pub use engine::tasks::{TaskInfo, TaskManager};
pub use output::AxumRegistryWriter;
pub use scanner::native::NativeAxumScanner;
pub use scanner::{get_scanner, get_scanner_with_spec, AxumScanner};

#[cfg(feature = "openapi")]
pub use scanner::openapi::OpenAPIScanner;

// Re-export apcore core types for convenience.
pub use apcore::cancel::CancelToken;
pub use apcore::module::{ModuleAnnotations, ModuleExample};
pub use apcore::{
    APCore, AlwaysDenyHandler, ApCoreEvent, ApprovalHandler, AutoApproveHandler, Config, Context,
    ErrorCode, EventEmitter, Executor, Identity, Module, ModuleError, Registry, SamplingStrategy,
    TracingMiddleware, ACL,
};

// Re-export toolkit types used in the public API.
pub use apcore_toolkit::ScannedModule;

#[cfg(feature = "cli")]
pub use apcore_toolkit::HTTPProxyRegistryWriter;

// Re-export apcore-cli types when the CLI feature is enabled.
#[cfg(feature = "cli")]
pub use client::CreateCliConfig;

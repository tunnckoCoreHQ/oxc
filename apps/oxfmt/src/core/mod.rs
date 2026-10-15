mod config;
pub mod embed;
mod format;
mod global_ignore;
mod language;
pub mod options;
pub mod oxfmtrc;
mod support;
pub mod utils;

#[cfg(feature = "napi")]
mod external_services;
#[cfg(target_os = "wasi")]
mod wasi_host;

#[cfg(any(feature = "napi", target_os = "wasi"))]
pub use config::JsConfigLoaderCb;
pub use config::{ConfigResolver, ConfigScopes, ResolveOutcome, config_discovery};
#[cfg(feature = "napi")]
pub use config::{JsLoadJsConfigCb, create_js_config_loader, resolve_for_api};
pub use format::{FormatPlan, FormatResult, SourceFormatter};
pub use global_ignore::{build_global_ignore_matchers, is_ignored, resolve_ignore_paths};
pub use language::Language;
#[cfg(feature = "napi")]
pub use support::classify_file;

#[cfg(feature = "napi")]
pub use external_services::{
    ExternalServices, JsFormatEmbeddedCb, JsFormatEmbeddedDocCb, JsFormatFileCb,
    JsInitExternalServicesCb, JsSortTailwindClassesCb,
};
#[cfg(target_os = "wasi")]
pub use wasi_host::{ExternalServices, create_js_config_loader};

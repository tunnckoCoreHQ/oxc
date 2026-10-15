//! `wasm32-wasip1` host bridge: the `oxlint_host` imports stand in for the JS callbacks which the
//! NAPI build gets from `src-js/cli.ts`.
//!
//! A call passes its arguments as UTF-8 byte slices and returns a status, `0` for success.
//! The host keeps the response bytes until `response_read` copies them into Rust memory.
//! The host can implement an asynchronous call (loading a config or a plugin) with JSPI.
//!
//! `src-js/wasi.ts` implements these imports.

use std::sync::Arc;

use oxc_diagnostics::OxcDiagnostic;
use oxc_linter::{
    ExternalLinter, LintFileFailure, LintFileOutput, LintFileSource, LoadPluginResult,
};
use oxc_span::{ModuleKind, SourceType};
use serde::Deserialize;

use crate::js_config::{JsConfigLoaderCb, parse_js_config_response};

#[link(wasm_import_module = "oxlint_host")]
unsafe extern "C" {
    /// JSON array of config file paths -> `loadJsConfigs` result JSON.
    fn load_js_configs(paths_ptr: *const u8, paths_len: usize) -> u32;
    /// JSON `{ url, pluginName, pluginNameIsAlias }` -> `loadPlugin` result JSON.
    fn load_plugin(request_ptr: *const u8, request_len: usize) -> u32;
    /// Options JSON -> nothing, or an error message.
    fn setup_rule_configs(options_ptr: *const u8, options_len: usize) -> u32;
    /// Lint a file with JS plugins -> `lintFile` result JSON, or nothing if there are no diagnostics.
    ///
    /// `request` is JSON `{ filePath, ruleIds, optionsIds, collectTimings, lang, sourceType, hasBom }`.
    /// `tokens` and `comments` are arrays of `Token` and `Comment` in their raw transfer layout.
    fn lint_file(
        request_ptr: *const u8,
        request_len: usize,
        settings_ptr: *const u8,
        settings_len: usize,
        globals_ptr: *const u8,
        globals_len: usize,
        source_ptr: *const u8,
        source_len: usize,
        tokens_ptr: *const u8,
        tokens_len: usize,
        comments_ptr: *const u8,
        comments_len: usize,
    ) -> u32;
    /// Workspace URI -> nothing, or an error message.
    fn create_workspace(uri_ptr: *const u8, uri_len: usize) -> u32;
    /// Workspace URI -> nothing, or an error message.
    fn destroy_workspace(uri_ptr: *const u8, uri_len: usize) -> u32;
    fn response_len() -> usize;
    fn response_read(dst_ptr: *mut u8);
}

/// Take the response of the last call.
fn response() -> String {
    // SAFETY: the host copies exactly `response_len()` bytes to the buffer.
    let bytes = unsafe {
        let mut bytes = vec![0; response_len()];
        response_read(bytes.as_mut_ptr());
        bytes
    };
    String::from_utf8(bytes).expect("host response should be UTF-8")
}

/// Call an import which takes one string. `Ok` holds the response, `Err` the error message.
fn call(f: unsafe extern "C" fn(*const u8, usize) -> u32, arg: &str) -> Result<String, String> {
    // SAFETY: the host only reads `arg` during the call.
    let status = unsafe { f(arg.as_ptr(), arg.len()) };
    let response = response();
    if status == 0 { Ok(response) } else { Err(response) }
}

/// JS/TS config loader: the host evaluates `oxlint.config.ts` files.
pub fn create_js_config_loader() -> JsConfigLoaderCb {
    Box::new(|paths: Vec<String>| {
        let paths = serde_json::Value::from(paths).to_string();
        match call(load_js_configs, &paths) {
            Ok(json) => parse_js_config_response(&json),
            Err(err) => {
                Err(vec![OxcDiagnostic::error(format!("`loadJsConfigs` threw an error: {err}"))])
            }
        }
    })
}

/// Result returned by `loadPlugin`.
#[derive(Deserialize)]
enum LoadPluginReturnValue {
    Success(LoadPluginResult),
    Failure(String),
}

/// Result returned by `lintFile`.
#[derive(Deserialize)]
enum LintFileReturnValue {
    Success(LintFileOutput),
    Failure(LintFileFailure),
}

/// [`ExternalLinter`] for JS plugins, which the host runs.
pub fn create_external_linter() -> ExternalLinter {
    let load_plugin = Arc::new(Box::new(
        |url: String,
         plugin_name: Option<String>,
         plugin_name_is_alias: bool,
         _: Option<String>| {
            let request = serde_json::json!({
                "url": url,
                "pluginName": plugin_name,
                "pluginNameIsAlias": plugin_name_is_alias,
            });
            let json = call(load_plugin, &request.to_string())
                .map_err(|err| format!("`loadPlugin` threw an error: {err}"))?;
            match serde_json::from_str(&json) {
                Ok(LoadPluginReturnValue::Success(result)) => Ok(result),
                Ok(LoadPluginReturnValue::Failure(err)) => Err(err),
                Err(err) => {
                    Err(format!("Failed to deserialize JSON returned by `loadPlugin`: {err}"))
                }
            }
        },
    ) as Box<_>);

    let setup_rule_configs = Arc::new(Box::new(|options_json: String| {
        call(setup_rule_configs, &options_json).map(|_| ())
    }) as Box<_>);

    let lint_file = Arc::new(Box::new(
        |file_path: String,
         rule_ids: Vec<u32>,
         options_ids: Vec<u32>,
         settings_json: String,
         globals_json: String,
         collect_timings: bool,
         _: Option<String>,
         source: &LintFileSource<'_>| {
            let request = serde_json::json!({
                "filePath": file_path,
                "ruleIds": rule_ids,
                "optionsIds": options_ids,
                "collectTimings": collect_timings,
                "lang": lang(source.source_type),
                "sourceType": module_kind(source.source_type),
                "hasBom": source.has_bom,
            })
            .to_string();
            // SAFETY: the host only reads the slices during the call.
            // `Token` and `Comment` are plain data, so their bytes are their raw transfer layout.
            let status = unsafe {
                lint_file(
                    request.as_ptr(),
                    request.len(),
                    settings_json.as_ptr(),
                    settings_json.len(),
                    globals_json.as_ptr(),
                    globals_json.len(),
                    source.source_text.as_ptr(),
                    source.source_text.len(),
                    source.tokens.as_ptr().cast(),
                    size_of_val(source.tokens),
                    source.comments.as_ptr().cast(),
                    size_of_val(source.comments),
                )
            };
            let json = response();
            if status != 0 {
                return Err(format!("`lintFile` threw an error: {json}").into());
            }
            // `lintFile` returns nothing if no diagnostics reported, and no error occurred
            if json.is_empty() {
                return Ok(LintFileOutput::default());
            }
            match serde_json::from_str(&json) {
                Ok(LintFileReturnValue::Success(output)) => Ok(output),
                Ok(LintFileReturnValue::Failure(failure)) => Err(failure),
                Err(err) => {
                    Err(format!("Failed to deserialize JSON returned by `lintFile`: {err}").into())
                }
            }
        },
    ) as Box<_>);

    let create_workspace = Arc::new(Box::new(|workspace_uri: String| {
        call(create_workspace, &workspace_uri).map(|_| ())
    }) as Box<_>);

    let destroy_workspace = Arc::new(Box::new(|workspace_uri: String| {
        call(destroy_workspace, &workspace_uri).map(|_| ())
    }) as Box<_>);

    ExternalLinter::new(
        load_plugin,
        setup_rule_configs,
        lint_file,
        create_workspace,
        destroy_workspace,
    )
}

/// oxc-parser's `lang` option for the file.
fn lang(source_type: SourceType) -> &'static str {
    match (
        source_type.is_typescript(),
        source_type.is_typescript_definition(),
        source_type.is_jsx(),
    ) {
        (true, true, _) => "dts",
        (true, false, true) => "tsx",
        (true, false, false) => "ts",
        (false, _, true) => "jsx",
        (false, _, false) => "js",
    }
}

/// oxc-parser's `sourceType` option for the file.
fn module_kind(source_type: SourceType) -> &'static str {
    match source_type.module_kind() {
        ModuleKind::Script => "script",
        ModuleKind::Module => "module",
        ModuleKind::Unambiguous => "unambiguous",
        ModuleKind::CommonJS => "commonjs",
    }
}

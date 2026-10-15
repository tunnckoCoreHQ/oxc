//! `wasm32-wasip1` host bridge: the `oxfmt_host` imports stand in for the JS callbacks which the
//! NAPI build gets from `src-js/cli.ts`, with the same Rust surface as `external_services.rs`.
//!
//! A call passes its arguments as UTF-8 byte slices and returns a status, `0` for success.
//! The host keeps the response bytes until `response_read` copies them into Rust memory.
//! The host can implement a call asynchronously (loading a config, running Prettier) with JSPI.
//!
//! `src-js/wasi.ts` implements these imports.

use std::sync::Arc;

use serde_json::Value;

use super::{
    JsConfigLoaderCb,
    embed::{
        FormatEmbeddedDocWithConfigCallback, FormatEmbeddedWithConfigCallback,
        TailwindWithConfigCallback,
    },
};

#[link(wasm_import_module = "oxfmt_host")]
unsafe extern "C" {
    /// Absolute config file path -> JSON of the config object, or `null` to skip the file
    /// (a Vite+ config without `.fmt`). An error message otherwise.
    fn load_js_config(path_ptr: *const u8, path_len: usize) -> u32;
    /// Prettier options JSON (with `parser` and `filepath`) and code -> formatted code,
    /// or an error message.
    fn format_file(
        options_ptr: *const u8,
        options_len: usize,
        code_ptr: *const u8,
        code_len: usize,
    ) -> u32;
    /// Prettier options JSON (with `parser`) and code -> formatted code, or a non-zero status.
    fn format_embedded(
        options_ptr: *const u8,
        options_len: usize,
        code_ptr: *const u8,
        code_len: usize,
    ) -> u32;
    /// Prettier options JSON (with `parser`) and code -> Prettier `Doc` JSON, or a non-zero status.
    fn format_embedded_doc(
        options_ptr: *const u8,
        options_len: usize,
        code_ptr: *const u8,
        code_len: usize,
    ) -> u32;
    /// Prettier options JSON (with `filepath`) and JSON array of classes
    /// -> JSON array of the sorted classes, or a non-zero status.
    fn sort_tailwind_classes(
        options_ptr: *const u8,
        options_len: usize,
        classes_ptr: *const u8,
        classes_len: usize,
    ) -> u32;
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

/// Call an import which takes two strings. `Ok` holds the response, `Err` the error message.
fn call(
    f: unsafe extern "C" fn(*const u8, usize, *const u8, usize) -> u32,
    a: &str,
    b: &str,
) -> Result<String, String> {
    // SAFETY: the host only reads `a` and `b` during the call.
    let status = unsafe { f(a.as_ptr(), a.len(), b.as_ptr(), b.len()) };
    let response = response();
    if status == 0 { Ok(response) } else { Err(response) }
}

/// JS/TS config loader: the host evaluates `oxfmt.config.ts` (or `vite.config.ts`) files.
pub fn create_js_config_loader() -> JsConfigLoaderCb {
    Arc::new(|path: String| {
        // SAFETY: the host only reads `path` during the call.
        let status = unsafe { load_js_config(path.as_ptr(), path.len()) };
        let json = response();
        if status != 0 {
            return Err(json);
        }
        serde_json::from_str(&json).map_err(|err| err.to_string())
    })
}

/// Callback function type for formatting whole files (Tier 3/4 delegation).
/// Takes (options, code) and returns formatted code or an error.
pub type FormatFileWithConfigCallback =
    Arc<dyn Fn(Value, &str) -> Result<String, String> + Send + Sync>;

/// Transport handle to the host-side services (Prettier formatting, Tailwind sorting),
/// each wrapped as a plain Rust callback.
pub struct ExternalServices {
    pub format_file: FormatFileWithConfigCallback,
    pub format_embedded: FormatEmbeddedWithConfigCallback,
    pub format_embedded_doc: FormatEmbeddedDocWithConfigCallback,
    pub sort_tailwindcss_classes: TailwindWithConfigCallback,
}

impl Default for ExternalServices {
    fn default() -> Self {
        Self {
            format_file: Arc::new(|options: Value, code: &str| {
                call(format_file, &options.to_string(), code)
            }),
            format_embedded: Arc::new(|options: Value, code: &str| {
                call(format_embedded, &options.to_string(), code)
                    .map_err(|_| "Embedded formatting failed".to_string())
            }),
            format_embedded_doc: Arc::new(|options: Value, code: &str| {
                call(format_embedded_doc, &options.to_string(), code)
                    .map_err(|_| "Embedded doc formatting failed".to_string())
            }),
            sort_tailwindcss_classes: Arc::new(|options: &Value, classes: Vec<String>| {
                let json = serde_json::to_string(&classes).expect("classes serialize to JSON");
                call(sort_tailwind_classes, &options.to_string(), &json)
                    .ok()
                    .and_then(|sorted| serde_json::from_str(&sorted).ok())
                    .unwrap_or(classes)
            }),
        }
    }
}

impl ExternalServices {
    /// Format non-js file using the host.
    /// The `options` Value should already have `parser` and `filepath` set by the caller.
    ///
    /// # Errors
    /// Returns the error message of the host, e.g. a Prettier syntax error.
    pub fn format_file(&self, options: Value, code: &str) -> Result<String, String> {
        (self.format_file)(options, code)
    }
}

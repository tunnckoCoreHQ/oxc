use std::io::BufWriter;

use oxlint::cli::{CliRunResult, CliRunner, init_tracing, lint_command};
#[cfg(not(target_os = "wasi"))]
use oxlint::lsp::run_lsp;

fn main() -> CliRunResult {
    // wasi-libc starts in `/`, and does not read `PWD`. Use the working directory the host gives.
    #[cfg(target_os = "wasi")]
    if let Some(pwd) = std::env::var_os("PWD") {
        std::env::set_current_dir(pwd).expect("Failed to change directory to `PWD`");
    }

    // Parse command line arguments from std::env::args()
    let command = lint_command().run();

    // Both LSP and CLI use `tracing` for logging
    init_tracing();

    // If --lsp flag is set, run the language server.
    //
    // The language server is the only path that needs an async runtime, so the Tokio runtime is
    // created here rather than via `#[tokio::main]`. The CLI lint path is fully synchronous
    // (Rayon-based); `#[tokio::main]` would otherwise spawn one idle Tokio worker thread per core
    // on every lint invocation.
    #[cfg(not(target_os = "wasi"))]
    if command.lsp {
        let runtime =
            tokio::runtime::Runtime::new().expect("Failed to build the Tokio runtime for the LSP");
        return runtime.block_on(async {
            run_lsp(
                None,
                #[cfg(feature = "napi")]
                None,
            )
            .await;
            CliRunResult::LintSucceeded
        });
    }

    command.handle_threads();

    // stdio is blocked by LineWriter, use a BufWriter to reduce syscalls.
    // See `https://github.com/rust-lang/rust/issues/60673`.
    let mut stdout = BufWriter::new(std::io::stdout());

    // Run without external linter (no JS plugins)
    #[cfg(not(target_os = "wasi"))]
    return CliRunner::new(command, None).run(&mut stdout);

    // On `wasm32-wasip1`, the host runs JS plugins and evaluates JS/TS configs
    #[cfg(target_os = "wasi")]
    CliRunner::new(command, Some(oxlint::cli::create_external_linter()))
        .with_config_loader(Some(oxlint::cli::create_js_config_loader()))
        .run(&mut stdout)
}

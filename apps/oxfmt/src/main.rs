use oxfmt::cli::{CliRunResult, WalkRunner, format_command, init_rayon, init_tracing};

// Pure Rust CLI entry point.
// This CLI only supports the basic `Cli` mode.
// For full featured JS CLI entry point, see `run_cli()` exported by `main_napi.rs`.

#[cfg(not(target_os = "wasi"))]
#[tokio::main]
async fn main() -> CliRunResult {
    // Parse command line arguments from std::env::args()
    let command = format_command().run();

    init_tracing();
    init_rayon(command.runtime_options.threads);
    WalkRunner::new(command).run()
}

// `wasm32-wasip1` CLI entry point: no threads, no async runtime.
// The host provides Prettier, Tailwind class sorting and JS/TS configs, see `core/wasi_host.rs`.
#[cfg(target_os = "wasi")]
fn main() -> CliRunResult {
    // wasi-libc starts in `/`, and does not read `PWD`. Use the working directory the host gives.
    if let Some(pwd) = std::env::var_os("PWD") {
        std::env::set_current_dir(pwd).expect("Failed to change directory to `PWD`");
    }

    // Parse command line arguments from std::env::args()
    let command = format_command().run();

    init_tracing();
    init_rayon(command.runtime_options.threads);
    WalkRunner::new(command)
        .with_external_services(Some(oxfmt::cli::ExternalServices::default()))
        .with_js_config_loader(oxfmt::cli::create_js_config_loader())
        .run()
}

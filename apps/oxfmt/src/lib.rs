#[cfg(feature = "napi")]
mod api;
pub mod cli;
mod core;
#[cfg(any(feature = "napi", target_os = "wasi"))]
mod prettier_compat;

pub use core::oxfmtrc;
#[cfg(feature = "napi")]
pub mod lsp;
#[cfg(feature = "napi")]
mod main_napi;
#[cfg(feature = "napi")]
pub use main_napi::*;

#[cfg(all(
    feature = "allocator",
    not(any(
        target_arch = "arm",
        target_arch = "riscv64",
        miri,
        target_os = "freebsd",
        target_family = "wasm"
    ))
))]
#[global_allocator]
static GLOBAL: mimalloc_safe::MiMalloc = mimalloc_safe::MiMalloc;

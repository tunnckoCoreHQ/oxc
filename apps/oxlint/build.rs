fn main() {
    // The plain `wasm32-wasip1` CLI is not a NAPI addon (NAPI on WASI needs emnapi).
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("wasi") {
        return;
    }
    napi_build::setup();
}

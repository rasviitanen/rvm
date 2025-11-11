use wasmtime::component::bindgen;

pub mod host;
pub mod quic;
pub mod state;

bindgen!({
    path: "./wit",
    world: "rvm-http",
    async: true,
    with: {
        "wasi:http/types@0.2.3": wasmtime_wasi_http::bindings::http::types,
        "wasi:http@0.2.3": wasmtime_wasi_http::bindings::http,
    }
});

use axum::body::Bytes;
use tokio::sync::{mpsc, oneshot};
use wasmtime::{
    component::{bindgen, Component},
    *,
};
use wasmtime_wasi::{IoView, ResourceTable, WasiCtx, WasiCtxBuilder, WasiView};
use wasmtime_wasi_http::{
    bindings::http::types::{ErrorCode, Scheme},
    body::HyperOutgoingBody,
    WasiHttpCtx, WasiHttpView,
};

// Generate bindings of the guest and host components.
bindgen!({
    path: "./wit",
    world: "rvm",
    async: true,
    with: {
        "wasi:http/types@0.2.3": wasmtime_wasi_http::bindings::http::types,
        "wasi:http@0.2.3": wasmtime_wasi_http::bindings::http,
    }
});

#[derive(Clone)]
struct HostComponent;

// Implementation of the host interface defined in the wit file.
impl rvm::lambda::host::Host for HostComponent {
    async fn multiply(&mut self, a: f32, b: f32) -> f32 {
        a * b
    }

    async fn client_secret(&mut self) -> String {
        String::from("THIS IS A SECRET!")
    }
}

pub struct RvmState {
    host: HostComponent,
    wasi: WasiCtx,
    http: WasiHttpCtx,
    table: ResourceTable,
}

impl IoView for RvmState {
    fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }
}
impl WasiView for RvmState {
    fn ctx(&mut self) -> &mut WasiCtx {
        &mut self.wasi
    }
}

impl WasiHttpView for RvmState {
    fn ctx(&mut self) -> &mut WasiHttpCtx {
        &mut self.http
    }
}

pub struct InvokeRequest {
    pub response: oneshot::Sender<Result<hyper::Response<HyperOutgoingBody>, ErrorCode>>,
    pub request: hyper::Request<hyper::body::Incoming>,
}

#[tracing::instrument(err, skip(engine, receiver, bytes))]
pub async fn compile_and_start_instance_worker(
    key: String,
    engine: &wasmtime::Engine,
    mut receiver: mpsc::UnboundedReceiver<InvokeRequest>,
    bytes: Bytes,
) -> Result<()> {
    // Load module and link components.
    // In production this should instead use a precompiled component.
    let mut linker = wasmtime::component::Linker::new(engine);
    rvm::lambda::host::add_to_linker(&mut linker, |state: &mut RvmState| &mut state.host)?;
    wasmtime_wasi_http::add_only_http_to_linker_async(&mut linker)?;
    wasmtime_wasi::add_to_linker_async(&mut linker)?;

    let component = Component::from_binary(engine, &bytes)?;
    let pre = RvmPre::new(linker.instantiate_pre(&component)?)?;

    // Create a store with limited fuel
    let mut store = Store::new(
        pre.engine(),
        RvmState {
            host: HostComponent,
            table: ResourceTable::new(),
            wasi: WasiCtxBuilder::new().inherit_stdio().build(),
            http: WasiHttpCtx::new(),
        },
    );
    store.set_fuel(100_000_000)?;

    // Instantiate and listen for requests
    let rvm = pre.instantiate_async(&mut store).await?;
    tokio::spawn(async move {
        while let Some(request) = receiver.recv().await {
            let uri = request.request.uri();
            tracing::info!(uri=%uri, "Invoking");

            let req = store
                .data_mut()
                .new_incoming_request(Scheme::Http, request.request)
                .unwrap();
            let (tx, rx) =
                oneshot::channel::<Result<hyper::Response<HyperOutgoingBody>, ErrorCode>>();
            let out = store.data_mut().new_response_outparam(tx).unwrap();

            let fuel_before = store.get_fuel().unwrap();

            let resp = rvm
                .wasi_http_incoming_handler()
                .call_handle(&mut store, req, out)
                .await;

            if let Err(e) = resp {
                if matches!(e.downcast::<Trap>(), Ok(Trap::OutOfFuel)) {
                    tracing::warn!("Fuel exhausted")
                }
                let _ = request.response.send(Err(ErrorCode::ConfigurationError));
                continue;
            };

            if let Ok(resp) = rx.await {
                let _ = request.response.send(resp.map(|mut r| {
                    let fuel_after = store.get_fuel().unwrap();
                    r.headers_mut()
                        .append("x-rvm-fuel-remaining", fuel_after.into());
                    r.headers_mut().append(
                        "x-rvm-fuel-consumed",
                        fuel_before.saturating_sub(fuel_after).into(),
                    );

                    r
                }));
            }
        }
    });
    Ok(())
}

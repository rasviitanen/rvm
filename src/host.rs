use axum::body::Bytes;
use tokio::sync::{mpsc, oneshot};
use tracing::info;

use wasmtime::{component::Component, Store, Trap};
use wasmtime_wasi::{ResourceTable, WasiCtx, WasiCtxBuilder, WasiView};
use wasmtime_wasi_http::{
    bindings::http::types::{ErrorCode, Scheme},
    body::HyperOutgoingBody,
    WasiHttpCtx, WasiHttpView,
};
use wasmtime_wasi_io::IoView;

use crate::{state::SharedState, RvmHttpPre};

// Implementation of the host interface defined in the wit file.
impl crate::rvm::lambda::host::Host for RvmState {
    async fn multiply(&mut self, a: f32, b: f32) -> f32 {
        a * b
    }

    async fn client_secret(&mut self) -> String {
        String::from("THIS IS A SECRET!")
    }

    async fn log(&mut self, line: String) {
        tracing::info!(line);
    }
}

pub struct RvmState {
    pub wasi: WasiCtx,
    pub http: WasiHttpCtx,
    pub table: ResourceTable,
}

impl RvmState {
    pub fn quic(&mut self) -> &mut ResourceTable {
        &mut self.table
    }
}

impl IoView for RvmState {
    fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }
}

impl WasiView for RvmState {
    fn ctx(&mut self) -> wasmtime_wasi::WasiCtxView<'_> {
        wasmtime_wasi::WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl WasiHttpView for RvmState {
    fn ctx(&mut self) -> &mut WasiHttpCtx {
        &mut self.http
    }

    fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }
}

pub struct InvokeRequest {
    pub response: oneshot::Sender<Result<hyper::Response<HyperOutgoingBody>, ErrorCode>>,
    pub request: hyper::Request<hyper::body::Incoming>,
}

#[tracing::instrument(err, skip(app, receiver, bytes))]
pub async fn compile_and_start_http(
    key: String,
    app: &SharedState,
    mut receiver: mpsc::UnboundedReceiver<InvokeRequest>,
    bytes: Bytes,
) -> anyhow::Result<()> {
    let component = Component::from_binary(&app.read().await.engine, &bytes)?;
    let pre: RvmHttpPre<RvmState> =
        RvmHttpPre::new(app.read().await.linker.instantiate_pre(&component)?)?;

    // Create a store with limited fuel
    let mut store = Store::new(
        pre.engine(),
        RvmState {
            table: ResourceTable::new(),
            wasi: WasiCtxBuilder::new().inherit_stdio().build(),
            http: WasiHttpCtx::new(),
        },
    );
    store.set_fuel(100_000_000)?;

    // Instantiate and listen for requests
    let rvm = pre.instantiate_async(&mut store).await?;
    tokio::spawn(async move {
        info!(key, "started instance worker");
        while let Some(request) = receiver.recv().await {
            let uri = request.request.uri();
            tracing::info!(uri=%uri, "Invoking");

            let req: wasmtime::component::Resource<wasmtime_wasi_http::types::HostIncomingRequest> =
                store
                    .data_mut()
                    .new_incoming_request(Scheme::Http, request.request)
                    .unwrap();
            let (tx, rx) =
                oneshot::channel::<Result<hyper::Response<HyperOutgoingBody>, ErrorCode>>();
            let out = store.data_mut().new_response_outparam(tx).unwrap();

            let fuel_before = store.get_fuel().unwrap();

            let resp = rvm
                .wasi_http_incoming_handler()
                .call_handle(&mut store, req, out);

            if let Err(e) = resp {
                tracing::debug!(%e, "invokation failed");
                if let Some(trap) = e.downcast_ref::<Trap>() {
                    if matches!(trap, Trap::OutOfFuel) {
                        tracing::warn!(key, "out of fuel");
                    }
                }
                let _ = request
                    .response
                    .send(Err(ErrorCode::InternalError(Some(e.to_string()))));
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
        info!(key, "stopped instance worker");
    });
    Ok(())
}

#[tracing::instrument(err, skip(app, bytes))]
pub async fn compile_and_start_quic(
    key: String,
    app: &SharedState,
    shutdown: oneshot::Receiver<()>,
    bytes: Bytes,
) -> anyhow::Result<()> {
    let component = Component::from_binary(&app.read().await.engine, &bytes)?;

    let mut store = Store::new(
        &app.read().await.engine,
        RvmState {
            table: ResourceTable::new(),
            wasi: WasiCtxBuilder::new().inherit_stdio().build(),
            http: WasiHttpCtx::new(),
        },
    );
    store.set_fuel(100_000_000)?;

    // Instantiate and listen for requests
    // let rvm = pre.instantiate_async(&mut store).await?;
    let command = wasmtime_wasi::p2::bindings::Command::instantiate_async(
        &mut store,
        &component,
        &app.read().await.linker,
    )
    .await?;
    tokio::spawn(async move {
        tokio::select! {
            _ = shutdown => {
                tracing::error!(key, "quic server closed by force")
            }
            program_result = command.wasi_cli_run().call_run(&mut store) => {
                if let Err(err) = program_result {
                    tracing::error!(key, %err, "quic server failed to run")
                } else if let Ok(Err(_)) = program_result {
                    tracing::error!(key, "quic server failed")
                } else {
                    tracing::info!(key, "quic server closed")
                }
            }
        }
    });
    Ok(())
}

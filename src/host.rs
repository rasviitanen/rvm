use axum::body::Bytes;
use opendal::EntryMode;
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

    async fn storage_put(&mut self, path: String, contents: Vec<u8>) -> Result<(), String> {
        let path = data_path("data", &path)?;
        self.storage
            .write(&path, contents)
            .await
            .map(|_| ())
            .map_err(|err| err.to_string())
    }

    async fn storage_get(&mut self, path: String) -> Result<Option<Vec<u8>>, String> {
        let path = data_path("data", &path)?;
        if !self
            .storage
            .exists(&path)
            .await
            .map_err(|err| err.to_string())?
        {
            return Ok(None);
        }

        self.storage
            .read(&path)
            .await
            .map(|buffer| Some(buffer.to_bytes().to_vec()))
            .map_err(|err| err.to_string())
    }

    async fn storage_list(&mut self, prefix: String) -> Result<Vec<String>, String> {
        let prefix = data_path("data", &prefix)?;
        list_files(&self.storage, &prefix, "data/").await
    }

    async fn kv_put(
        &mut self,
        namespace: String,
        key: String,
        value: String,
    ) -> Result<(), String> {
        let path = kv_path(&namespace, &key)?;
        self.storage
            .write(&path, value)
            .await
            .map(|_| ())
            .map_err(|err| err.to_string())
    }

    async fn kv_get(&mut self, namespace: String, key: String) -> Result<Option<String>, String> {
        let path = kv_path(&namespace, &key)?;
        if !self
            .storage
            .exists(&path)
            .await
            .map_err(|err| err.to_string())?
        {
            return Ok(None);
        }

        self.storage
            .read(&path)
            .await
            .map_err(|err| err.to_string())
            .and_then(|buffer| {
                String::from_utf8(buffer.to_bytes().to_vec()).map_err(|err| err.to_string())
            })
            .map(Some)
    }

    async fn kv_list(
        &mut self,
        namespace: String,
        prefix: String,
    ) -> Result<Vec<(String, String)>, String> {
        let namespace = clean_segment(&namespace)?;
        let key_prefix = clean_relative_path(&prefix)?;
        let path_prefix = if key_prefix.is_empty() {
            format!("kv/{namespace}/")
        } else {
            format!("kv/{namespace}/{key_prefix}")
        };
        let entries = self
            .storage
            .list(&path_prefix)
            .await
            .map_err(|err| err.to_string())?;
        let mut values = Vec::new();
        for entry in entries {
            if !matches!(entry.metadata().mode(), EntryMode::FILE) {
                continue;
            }
            let value = self
                .storage
                .read(entry.path())
                .await
                .map_err(|err| err.to_string())
                .and_then(|buffer| {
                    String::from_utf8(buffer.to_bytes().to_vec()).map_err(|err| err.to_string())
                })?;
            let key = entry
                .path()
                .trim_start_matches(&format!("kv/{namespace}/"))
                .trim_end_matches(".json")
                .to_owned();
            values.push((key, value));
        }
        values.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(values)
    }
}

pub struct RvmState {
    pub wasi: WasiCtx,
    pub http: WasiHttpCtx,
    pub table: ResourceTable,
    pub storage: opendal::Operator,
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
            storage: app.read().await.storage.clone(),
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
            storage: app.read().await.storage.clone(),
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

async fn list_files(
    storage: &opendal::Operator,
    prefix: &str,
    strip_prefix: &str,
) -> Result<Vec<String>, String> {
    let entries = storage.list(prefix).await.map_err(|err| err.to_string())?;
    let mut paths = Vec::new();
    for entry in entries {
        if matches!(entry.metadata().mode(), EntryMode::FILE) {
            paths.push(entry.path().trim_start_matches(strip_prefix).to_owned());
        }
    }
    paths.sort();
    Ok(paths)
}

fn data_path(root: &str, path: &str) -> Result<String, String> {
    let path = clean_relative_path(path)?;
    if path.is_empty() {
        Ok(format!("{root}/"))
    } else {
        Ok(format!("{root}/{path}"))
    }
}

fn kv_path(namespace: &str, key: &str) -> Result<String, String> {
    let namespace = clean_segment(namespace)?;
    let key = clean_relative_path(key)?;
    if key.is_empty() {
        return Err("key must not be empty".to_owned());
    }
    Ok(format!("kv/{namespace}/{key}.json"))
}

fn clean_segment(segment: &str) -> Result<String, String> {
    let segment = segment.trim_matches('/');
    if segment.is_empty()
        || segment.contains("..")
        || segment.contains('\\')
        || segment.contains('/')
    {
        return Err(format!("invalid path segment: {segment}"));
    }
    Ok(segment.to_owned())
}

fn clean_relative_path(path: &str) -> Result<String, String> {
    let path = path.trim_matches('/');
    if path.contains("..") || path.contains('\\') {
        return Err(format!("invalid relative path: {path}"));
    }
    Ok(path.to_owned())
}

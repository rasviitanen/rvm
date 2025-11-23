use std::{collections::HashMap, sync::Arc};

use futures::{StreamExt, TryStreamExt};
use opendal::EntryMode;
use tokio::sync::{mpsc, oneshot, RwLock};
use wasmtime::{component::{HasSelf}, *};

use crate::{host::{InvokeRequest, RvmState}, quic::QuicComponent};

pub enum ModuleInstance {
    Quic(oneshot::Sender<()>),
    Http(tokio::sync::mpsc::UnboundedSender<InvokeRequest>),
}

pub struct AppState {
    pub engine: wasmtime::Engine,
    pub instances: HashMap<String, ModuleInstance>,
    pub storage: opendal::Operator,
    pub linker: wasmtime::component::Linker<RvmState>,
}

#[derive(Clone)]
pub struct SharedState {
    inner: Arc<RwLock<AppState>>,
}

impl From<AppState> for SharedState {
    fn from(state: AppState) -> Self {
        Self {
            inner: Arc::new(RwLock::new(state)),
        }
    }
}

impl SharedState {
    pub async fn new() -> Result<SharedState> {
        let mut config = Config::new();
        // Enable the compilation cache, using the default cache configuration
        // settings.
        // config.cache_config_load_default()?;
        config.async_support(true);
        config.debug_info(true);
        config.wasm_backtrace_details(WasmBacktraceDetails::Enable);

        // Configure and enable the pooling allocator with space for 100 memories of
        // up to 268 KiB in size, 100 tables holding up to 10000 elements, and with a
        // limit of no more than 100 concurrent instances.
        let mut pool = PoolingAllocationConfig::new();
        pool.total_memories(100);
        pool.max_memory_size(1 << 28); // ~268KiB
        pool.total_tables(100);
        pool.table_elements(10_000);
        pool.total_core_instances(100);

        config.allocation_strategy(InstanceAllocationStrategy::Pooling(pool));
        config.memory_init_cow(true);
        config.consume_fuel(true);

        // Create an engine with our configuration.
        let engine = Engine::new(&config)?;

        // Create an opendal operator for publishing wasm modules
        // We use opendal so you can pick your backing store as you like.
        // For this demo, we use a simple filesystem, but could use redis, gcs, tikv etc.
        // Just switch the service here for something else.
        let builder = opendal::services::Fs::default().root("./module-store");
        let storage: opendal::Operator = opendal::Operator::new(builder)?.finish();

        let mut linker = wasmtime::component::Linker::new(&engine);
        crate::rvm::lambda::host::add_to_linker::<_, HasSelf<_>>(&mut linker, |state: &mut RvmState| state)?;
        crate::quic::rvm::lambda::quic::add_to_linker::<_, QuicComponent>(&mut linker, |state: &mut RvmState| state.quic())?;
        wasmtime_wasi_http::add_only_http_to_linker_async(&mut linker)?;
        wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;

        let state = AppState {
            engine,
            instances: Default::default(),
            storage,
            linker,
        };
        let http_modules = state.storage.list("http/").await?;
        let quic_modules = state.storage.list("quic/").await?;

        enum ModuleEntry {
            Quic(opendal::Entry),
            Http(opendal::Entry),
        }

        let state: SharedState = state.into();
        futures::stream::iter(
            http_modules
                .into_iter()
                .map(ModuleEntry::Http)
                .chain(quic_modules.into_iter().map(ModuleEntry::Quic)),
        )
        .map(Ok)
        .try_for_each_concurrent(16, |module_entry| {
            let state = state.clone();
            async move {
                let entry = match &module_entry {
                    ModuleEntry::Quic(entry) => entry,
                    ModuleEntry::Http(entry) => entry,
                };
                if !matches!(entry.metadata().mode(), EntryMode::FILE) {
                    return Ok(());
                }
                let module = state
                    .read()
                    .await
                    .storage
                    .read(entry.path())
                    .await?
                    .to_bytes();
                tracing::info!(
                    key = entry.name(),
                    bytes = module.len(),
                    "module downloaded successfully"
                );
                let hash = blake3::hash(&module);

                let name = entry.name().trim_end_matches(".wasm").to_owned();

                match module_entry {
                    ModuleEntry::Quic(entry) => {
                        let (tx, rx) = oneshot::channel();
                        tracing::info!(
                            key = entry.name(),
                            hash = %hash,
                            "restarting quic module",
                        );

                        crate::host::compile_and_start_quic(name.clone(), &state, rx, module)
                            .await?;
                        state
                            .write()
                            .await
                            .instances
                            .insert(name, ModuleInstance::Quic(tx));
                    }
                    ModuleEntry::Http(entry) => {
                        let (tx, rx) = mpsc::unbounded_channel();

                        tracing::info!(
                            key = entry.name(),
                            hash = %hash,
                            "restarting http module",
                        );

                        crate::host::compile_and_start_http(name.clone(), &state, rx, module)
                            .await?;
                        state
                            .write()
                            .await
                            .instances
                            .insert(name, ModuleInstance::Http(tx));
                    }
                }

                Ok::<_, anyhow::Error>(())
            }
        })
        .await?;

        Ok(state)
    }

    pub async fn read(&self) -> tokio::sync::RwLockReadGuard<'_, AppState> {
        self.inner.read().await
    }

    pub async fn write(&self) -> tokio::sync::RwLockWriteGuard<'_, AppState> {
        self.inner.write().await
    }
}

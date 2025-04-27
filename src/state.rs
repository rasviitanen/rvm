use std::{collections::HashMap, sync::Arc};

use opendal::EntryMode;
use tokio::sync::{mpsc, RwLock};
use wasmtime::*;

use crate::{compile_and_start_instance_worker, InvokeRequest};

pub type SharedState = Arc<RwLock<AppState>>;
pub struct AppState {
    pub engine: wasmtime::Engine,
    pub instances: HashMap<String, tokio::sync::mpsc::UnboundedSender<InvokeRequest>>,
    pub storage: opendal::Operator,
}

impl AppState {
    pub async fn new() -> Result<AppState> {
        let mut config = Config::new();
        // Enable the compilation cache, using the default cache configuration
        // settings.
        config.cache_config_load_default()?;
        config.async_support(true);

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
        let mut state = AppState {
            engine,
            instances: Default::default(),
            storage,
        };

        for module_entry in state.storage.list("").await? {
            if !matches!(module_entry.metadata().mode(), EntryMode::FILE) {
                continue;
            }
            // FIXME:(rasviitanen) run this concurrently
            let module = state.storage.read(module_entry.path()).await?.to_bytes();
            tracing::info!("Downloaded {} bytes", module.len());
            let (tx, rx) = mpsc::unbounded_channel();
            let hash = blake3::hash(&module);

            let name = module_entry.name().trim_end_matches(".wasm").to_owned();
            tracing::info!(
                "Restarting previously deployed module `{}` with hash {}",
                module_entry.name(),
                hash,
            );
            compile_and_start_instance_worker(name.clone(), &state.engine, rx, module).await?;
            state.instances.insert(name, tx);
        }

        Ok(state)
    }
}

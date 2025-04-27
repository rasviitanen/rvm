use std::{str::FromStr, sync::Arc};

use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Path, State},
    handler::Handler,
    http::{uri::PathAndQuery, StatusCode},
    routing::post_service,
    Json, Router,
};
use hyper::{server::conn::http1, Uri};
use tokio::sync::{oneshot, RwLock};
use tower_http::limit::RequestBodyLimitLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use wasmtime::*;
use wasmtime_wasi_http::{bindings::http::types::ErrorCode, body::HyperOutgoingBody, io::TokioIo};

mod host;
mod state;

use crate::host::*;
use crate::state::*;

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| format!("{}=debug", env!("CARGO_CRATE_NAME")).into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let state = Arc::new(RwLock::new(
        AppState::new().await.expect("failed to init state"),
    ));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:8000")
        .await
        .expect("Failed to setup listener");
    tracing::info!(
        "Listening for invokations on {}",
        listener.local_addr().expect("Failed to listen on addres")
    );

    // Start a hyper server to listen for invokations
    let service_fn = move |state: SharedState| {
        hyper::service::service_fn(move |mut req| {
            let state = state.clone();
            async move {
                // Strip the first part of the path and use it as the identifier for the instance.
                // A real app should probably use a host and subdomain to specify module.
                let mut uri_parts = req.uri().clone().into_parts();
                if let Some(path_and_query) = &mut uri_parts.path_and_query {
                    let path_and_query_string = path_and_query.to_string();
                    let (key, forward) = path_and_query_string
                        .trim_start_matches('/')
                        .split_once('/')
                        .map(|(p, q)| (p.to_owned(), q.to_owned()))
                        .unwrap_or_else(|| {
                            (
                                path_and_query.path().to_owned(),
                                path_and_query.query().unwrap_or("").to_owned(),
                            )
                        });
                    let Ok(new_uri) = PathAndQuery::from_str(&format!("/{forward}"))
                        .map_err(|_| StatusCode::BAD_REQUEST)
                        .and_then(|q| {
                            uri_parts.path_and_query = Some(q);
                            Uri::from_parts(uri_parts).map_err(|_| StatusCode::BAD_REQUEST)
                        })
                    else {
                        return hyper::Response::builder()
                            .status(StatusCode::BAD_REQUEST)
                            .body(Default::default());
                    };
                    *req.uri_mut() = new_uri;

                    tracing::info!(key=%key, "Invoking module");
                    return match services::invoke_module(&key, req, state).await {
                        Ok(ok) => Ok(ok),
                        Err(code) => hyper::Response::builder()
                            .status(code)
                            .body(Default::default()),
                    };
                }

                hyper::Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .body(Default::default())
            }
        })
    };
    let state_clone = state.clone();
    let serve_proxy = async move {
        let state = state_clone;
        loop {
            let (client, addr) = listener
                .accept()
                .await
                .expect("failed to accept connection");
            let state = state.clone();
            tokio::task::spawn(async move {
                if let Err(e) = http1::Builder::new()
                    .keep_alive(true)
                    .serve_connection(TokioIo::new(client), service_fn(state))
                    .await
                {
                    tracing::error!("error serving client[{addr}]: {e:?}");
                }
            });
        }
    };

    // Start an axum server to act as an admin service
    let listener_axum = tokio::net::TcpListener::bind("127.0.0.1:8002")
        .await
        .unwrap();

    tracing::info!(
        "Listening for deployments on {}",
        listener_axum
            .local_addr()
            .expect("Failed to listen on addres")
    );

    // build our application with a route
    let app = Router::new().route(
        "/deploy/{key}",
        post_service(
            services::deploy_module
                .layer((
                    DefaultBodyLimit::disable(),
                    RequestBodyLimitLayer::new(1024 * 256_000 /* ~256mb */),
                ))
                .with_state(state),
        ),
    );
    let serve_admin = axum::serve(listener_axum, app);
    let (admin_res, proxy_res): (Result<(), std::io::Error>, Result<(), std::io::Error>) =
        tokio::join!(serve_admin, serve_proxy);
    admin_res.expect("admin service failed");
    proxy_res.expect("invoke service failed");
}

mod services {
    use super::*;

    #[tracing::instrument(skip(state, request))]
    pub async fn invoke_module(
        key: &str,
        request: hyper::Request<hyper::body::Incoming>,
        state: SharedState,
    ) -> Result<hyper::Response<HyperOutgoingBody>, StatusCode> {
        let (tx, rx) = oneshot::channel::<Result<hyper::Response<HyperOutgoingBody>, ErrorCode>>();
        {
            let state = state.read().await;
            let state = state.instances.get(key).ok_or(StatusCode::NOT_FOUND)?;
            state
                .send(InvokeRequest {
                    response: tx,
                    request,
                })
                .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        }
        match rx.await {
            Ok(Ok(resp)) => Ok(resp),
            Ok(Err(_)) => Err(StatusCode::INTERNAL_SERVER_ERROR),
            Err(_) => Err(StatusCode::SERVICE_UNAVAILABLE),
        }
    }

    #[derive(serde::Serialize)]
    pub struct DeployResponse {
        hash: String,
    }

    #[tracing::instrument(skip(state, bytes))]
    pub async fn deploy_module(
        Path(key): Path<String>,
        State(state): State<SharedState>,
        bytes: Bytes,
    ) -> Result<Json<DeployResponse>, StatusCode> {
        let hash = blake3::hash(&bytes);
        let mut state = state.write().await;
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

        
        // Worker gets killed when tx is dropped
        compile_and_start_instance_worker(key.clone(), &state.engine, rx, bytes.clone())
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        // Upload
        let storage = state.storage.clone();
        let module_name = format!("{key}.wasm");
        tokio::spawn(async move { 
            let mut w = storage.writer(&module_name).await?;
            let len = bytes.len();
            w.write(bytes).await?;
            tracing::info!("Uploaded {len} bytes");
            w.close().await?;
            Ok::<_, anyhow::Error>(())
         })
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        state.instances.insert(key, tx);

        Ok(DeployResponse {
            hash: hash.to_string(),
        }
        .into())
    }
}

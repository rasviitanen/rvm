use bytes::Bytes;
use quinn::{Connection, Endpoint, VarInt};
use wasmtime::component::bindgen;
use wasmtime_wasi::p2::{
    bindings::sockets::network::{IpSocketAddress, Ipv4SocketAddress, Ipv6SocketAddress},
    pipe::{AsyncReadStream, AsyncWriteStream},
    DynInputStream, DynOutputStream, DynPollable, Pollable,
};

use crate::{
    host::RvmState,
    quic::rvm::lambda::quic::{ErrorCode, ShutdownType},
};

bindgen!({
    path: "./wit",
    world: "rvm-quic",
    with: {
        "rvm:lambda/quic@0.1.0/quic-socket":  QuicSocket,
        "wasi:io/poll@0.2.8":  wasmtime_wasi::p2::bindings::io::poll,
        "wasi:io/error@0.2.8":  wasmtime_wasi::p2::bindings::io::error,
        "wasi:io/streams@0.2.8":  wasmtime_wasi::p2::bindings::io::streams,
        "wasi:clocks/monotonic-clock@0.2.8":  wasmtime_wasi::p2::bindings::clocks::monotonic_clock,
        "wasi:sockets/network@0.2.8":  wasmtime_wasi::p2::bindings::sockets::network,
    },
    imports: {
        // "rvm:lambda/quic@0.1.0/[method]quic-socket.subscribe": trappable,
        default: async,
    },
});

#[derive(Clone)]
pub struct QuicSocket {
    endpoint: Endpoint,
    conn: Option<Connection>,
}

impl Drop for QuicSocket {
    fn drop(&mut self) {
        tracing::warn!("QuicSocket dropped!");
    }
}

#[async_trait::async_trait]
impl Pollable for QuicSocket {
    async fn ready(&mut self) {
        tokio::task::yield_now().await;
    }
}

impl rvm::lambda::quic::Host for RvmState {
    fn create_quic_socket(
        &mut self,
        ip: IpSocketAddress,
    ) -> impl ::core::future::Future<
        Output = Result<wasmtime::component::Resource<QuicSocket>, ErrorCode>,
    > + Send {
        fn sockaddr_from_wasi(addr: IpSocketAddress) -> std::net::SocketAddr {
            match addr {
                IpSocketAddress::Ipv4(Ipv4SocketAddress { address, port }) => {
                    std::net::SocketAddr::V4(std::net::SocketAddrV4::new(
                        std::net::Ipv4Addr::new(address.0, address.1, address.2, address.3),
                        port,
                    ))
                }
                IpSocketAddress::Ipv6(Ipv6SocketAddress {
                    address,
                    port,
                    flow_info,
                    scope_id,
                }) => std::net::SocketAddr::V6(std::net::SocketAddrV6::new(
                    std::net::Ipv6Addr::new(
                        address.0, address.1, address.2, address.3, address.4, address.5,
                        address.6, address.7,
                    ),
                    port,
                    flow_info,
                    scope_id,
                )),
            }
        }

        let server_addr = sockaddr_from_wasi(ip);
        let (endpoint, _server_cert) = internal::make_server_endpoint(server_addr).expect("failed");
        let socket = self.table.push(QuicSocket {
            endpoint,
            conn: None,
        });
        async move {
            tracing::info!(%server_addr, "new quic socket created");
            let socket = socket.unwrap();
            Ok(socket)
        }
    }
}

impl rvm::lambda::quic::HostQuicSocket for RvmState {
    fn accept(
        &mut self,
        socket: wasmtime::component::Resource<QuicSocket>,
    ) -> impl ::core::future::Future<
        Output = Result<wasmtime::component::Resource<QuicSocket>, ErrorCode>,
    > + ::core::marker::Send {
        async move {
            let endpoint = match self.table.get::<QuicSocket>(&socket) {
                Ok(s) => s.endpoint.clone(),
                Err(err) => {
                    tracing::error!(%err, "invalid or dropped socket");
                    return Err(ErrorCode::InvalidArgument);
                }
            };

            let local_addr = match endpoint.local_addr() {
                Ok(addr) => addr,
                Err(err) => {
                    tracing::error!(%err, "failed to get local addr");
                    return Err(ErrorCode::Unknown);
                }
            };
            tracing::info!(%local_addr, "waiting for new connection");
            if let Some(incoming) = endpoint.accept().await {
                tracing::info!("accepted new connection");
                match incoming.await {
                    Ok(conn) => {
                        let conn_socket = self
                            .table
                            .push(QuicSocket {
                                endpoint,
                                conn: Some(conn),
                            })
                            .expect("to push connection socket");
                        tracing::info!("connection ready");
                        return Ok(conn_socket);
                    }
                    Err(err) => tracing::error!(%err, "failed to await incoming connection"),
                }
            }
            Err(ErrorCode::Unknown)
        }
    }

    fn accept_bidirectional_stream(
        &mut self,
        socket: wasmtime::component::Resource<QuicSocket>,
    ) -> impl ::core::future::Future<
        Output = Result<
            (
                wasmtime::component::Resource<DynInputStream>,
                wasmtime::component::Resource<DynOutputStream>,
            ),
            ErrorCode,
        >,
    > + ::core::marker::Send {
        async move {
            let conn = match self.table.get::<QuicSocket>(&socket) {
                Ok(s) => match &s.conn {
                    Some(conn) => conn.clone(),
                    None => return Err(ErrorCode::InvalidState),
                },
                Err(err) => {
                    tracing::error!(%err, "invalid or dropped socket");
                    return Err(ErrorCode::InvalidArgument);
                }
            };

            let (send, recv) = match conn.accept_bi().await {
                Ok(val) => val,
                Err(err) => {
                    tracing::error!(%err, "accept_bi failed");
                    return Err(ErrorCode::ConnectionAborted);
                }
            };

            let input = self
                .table
                .push_child(
                    Box::new(AsyncReadStream::new(recv)) as DynInputStream,
                    &socket,
                )
                .expect("to push input child");

            let output = self
                .table
                .push_child(
                    Box::new(AsyncWriteStream::new(64 * 1024, send)) as DynOutputStream,
                    &socket,
                )
                .expect("to push output child");

            Ok((input, output))
        }
    }

    fn send_datagram(
        &mut self,
        socket: wasmtime::component::Resource<QuicSocket>,
        payload: Vec<u8>,
    ) -> impl ::core::future::Future<Output = Result<(), ErrorCode>> + ::core::marker::Send {
        let conn = self
            .table
            .get::<QuicSocket>(&socket)
            .ok()
            .and_then(|socket| socket.conn.clone());

        async move {
            let Some(conn) = conn else {
                return Err(ErrorCode::InvalidState);
            };

            conn.send_datagram(Bytes::from(payload))
                .map_err(map_datagram_send_error)
        }
    }

    fn receive_datagram(
        &mut self,
        socket: wasmtime::component::Resource<QuicSocket>,
    ) -> impl ::core::future::Future<Output = Result<Vec<u8>, ErrorCode>> + ::core::marker::Send
    {
        let conn = self
            .table
            .get::<QuicSocket>(&socket)
            .ok()
            .and_then(|socket| socket.conn.clone());

        async move {
            let Some(conn) = conn else {
                return Err(ErrorCode::InvalidState);
            };

            conn.read_datagram()
                .await
                .map(|bytes| bytes.to_vec())
                .map_err(|err| {
                    tracing::error!(%err, "failed to receive datagram");
                    ErrorCode::ConnectionAborted
                })
        }
    }

    fn max_datagram_size(
        &mut self,
        socket: wasmtime::component::Resource<QuicSocket>,
    ) -> impl ::core::future::Future<Output = Option<u64>> + ::core::marker::Send {
        let size = self
            .table
            .get::<QuicSocket>(&socket)
            .ok()
            .and_then(|socket| socket.conn.as_ref())
            .and_then(Connection::max_datagram_size)
            .map(|size| size as u64);

        async move { size }
    }

    fn subscribe(
        &mut self,
        socket: wasmtime::component::Resource<QuicSocket>,
    ) -> impl ::core::future::Future<Output = wasmtime::component::Resource<DynPollable>> {
        tracing::warn!("CALLED SUBSCRIBE");
        async move { wasmtime_wasi::p2::subscribe(&mut self.table, socket).unwrap() }
    }

    fn local_address(
        &mut self,
        socket: wasmtime::component::Resource<QuicSocket>,
    ) -> impl ::core::future::Future<Output = Result<IpSocketAddress, ErrorCode>> + ::core::marker::Send
    {
        fn sockaddr_to_wasi(addr: std::net::SocketAddr) -> IpSocketAddress {
            match addr {
                std::net::SocketAddr::V4(addr) => {
                    let ip = addr.ip().octets();
                    IpSocketAddress::Ipv4(Ipv4SocketAddress {
                        address: (ip[0], ip[1], ip[2], ip[3]),
                        port: addr.port(),
                    })
                }
                std::net::SocketAddr::V6(addr) => {
                    let ip = addr.ip().segments();
                    IpSocketAddress::Ipv6(Ipv6SocketAddress {
                        address: (ip[0], ip[1], ip[2], ip[3], ip[4], ip[5], ip[6], ip[7]),
                        port: addr.port(),
                        flow_info: addr.flowinfo(),
                        scope_id: addr.scope_id(),
                    })
                }
            }
        }

        let socket = self.table.get(&socket).cloned();
        async move {
            if let Ok(socket) = socket {
                return socket
                    .endpoint
                    .local_addr()
                    .map(sockaddr_to_wasi)
                    .map_err(|_| ErrorCode::InvalidState);
            }
            Err(ErrorCode::InvalidState)
        }
    }

    fn shutdown(
        &mut self,
        socket: wasmtime::component::Resource<QuicSocket>,
        _shutdown_type: ShutdownType,
    ) -> impl ::core::future::Future<Output = Result<(), ErrorCode>> + ::core::marker::Send {
        tracing::warn!("CALLED SHUTDOWN");
        let socket = self.table.delete(socket);
        async move {
            if let Ok(socket) = socket {
                if let Some(conn) = &socket.conn {
                    conn.close(VarInt::from_u32(0), b"server closed");
                } else {
                    socket.endpoint.close(VarInt::from_u32(0), b"server closed");
                }
            }
            Ok(())
        }
    }

    fn drop(
        &mut self,
        rep: wasmtime::component::Resource<QuicSocket>,
    ) -> impl ::core::future::Future<Output = wasmtime::Result<()>> + ::core::marker::Send {
        tracing::warn!("CALLED DROP");
        let _ = self.table.delete(rep);
        async move { Ok(()) }
    }
}

fn map_datagram_send_error(err: quinn::SendDatagramError) -> ErrorCode {
    tracing::error!(%err, "failed to send datagram");
    match err {
        quinn::SendDatagramError::UnsupportedByPeer | quinn::SendDatagramError::Disabled => {
            ErrorCode::NotSupported
        }
        quinn::SendDatagramError::TooLarge => ErrorCode::InvalidArgument,
        quinn::SendDatagramError::ConnectionLost(_) => ErrorCode::ConnectionAborted,
    }
}

mod internal {
    use quinn::{Endpoint, ServerConfig};
    use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
    use std::{error::Error, net::SocketAddr, sync::Arc};

    #[allow(unused)]
    pub fn make_server_endpoint(
        bind_addr: SocketAddr,
    ) -> Result<(Endpoint, CertificateDer<'static>), Box<dyn Error + Send + Sync + 'static>> {
        let (server_config, server_cert) = configure_server()?;
        let endpoint = Endpoint::server(server_config, bind_addr)?;
        Ok((endpoint, server_cert))
    }

    fn configure_server(
    ) -> Result<(ServerConfig, CertificateDer<'static>), Box<dyn Error + Send + Sync + 'static>>
    {
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let cert_der = CertificateDer::from(cert.cert);
        let priv_key = PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der());

        let mut server_config =
            ServerConfig::with_single_cert(vec![cert_der.clone()], priv_key.into())?;
        let transport_config = Arc::get_mut(&mut server_config.transport).unwrap();
        transport_config.max_concurrent_bidi_streams(256_u32.into());
        transport_config.max_concurrent_uni_streams(256_u32.into());
        transport_config.datagram_receive_buffer_size(Some(1024 * 1024));
        transport_config.datagram_send_buffer_size(1024 * 1024);

        Ok((server_config, cert_der))
    }
}

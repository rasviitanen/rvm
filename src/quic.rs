use std::sync::Arc;

use quinn::{Endpoint, VarInt};
use tokio::sync::Mutex;
use wasmtime::component::{bindgen};
use wasmtime_wasi::{ResourceTable, bindings::sockets::network::{IpSocketAddress, Ipv4SocketAddress, Ipv6SocketAddress}, pipe::{AsyncReadStream, AsyncWriteStream}};

use crate::quic::rvm::lambda::quic::{
    ErrorCode, InputStream, OutputStream, Pollable, ShutdownType
};

bindgen!({
    path: "./wit",
    world: "rvm-quic",
    async: true,
    with: {
        "rvm:lambda/quic@0.1.0/quic-socket":  QuicSocket,
        "wasi:io/poll@0.2.3":  wasmtime_wasi::bindings::io::poll,
        "wasi:io/error@0.2.3":  wasmtime_wasi::bindings::io::error,
        "wasi:io/streams@0.2.3":  wasmtime_wasi::bindings::io::streams,
        "wasi:clocks/monotonic-clock@0.2.3":  wasmtime_wasi::bindings::clocks::monotonic_clock,
        "wasi:sockets/network@0.2.3":  wasmtime_wasi::bindings::sockets::network,
    },
});

pub struct QuicSocket {
    endpoint: Endpoint,
}

#[async_trait::async_trait]
impl wasmtime_wasi::Pollable for QuicSocket {
   async fn ready(&mut self) {
        // todo!();
    }
}

#[derive(Default)]
pub struct QuicComponent {
    pub table: Arc<Mutex<ResourceTable>>,
}

impl rvm::lambda::quic::Host for QuicComponent {
    fn create_quic_socket(
        &mut self,
        ip: IpSocketAddress,
    ) -> impl ::core::future::Future<
        Output = Result<wasmtime::component::Resource<QuicSocket>, ErrorCode>,
    > + ::core::marker::Send {
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
                        address.0, address.1, address.2, address.3, address.4, address.5, address.6,
                        address.7,
                    ),
                    port,
                    flow_info,
                    scope_id,
                )),
            }
        }

        let server_addr = sockaddr_from_wasi(ip);
        let (endpoint, _server_cert) = internal::make_server_endpoint(server_addr).expect("failed");
        let table = self.table.clone();
        async move { 
            let socket = table.lock().await.push(QuicSocket {
                endpoint
            });
            tracing::info!("new quic socket created");
            let socket = socket.unwrap();
            Ok(socket)
         }
    }
}

impl rvm::lambda::quic::HostQuicSocket for QuicComponent {
    fn accept(
        &mut self,
        socket: wasmtime::component::Resource<QuicSocket>,
    ) -> impl ::core::future::Future<
        Output = Result<
            (
                wasmtime::component::Resource<QuicSocket>,
                wasmtime::component::Resource<InputStream>,
                wasmtime::component::Resource<OutputStream>,
            ),
            ErrorCode,
        >,
    > + ::core::marker::Send {
        let table = self.table.clone();
        async move { 
            let mut table = table.lock().await;
            let s: &QuicSocket = table.get::<QuicSocket>(&socket).expect("socket is destroyed");
            let local_addr = s.endpoint.local_addr().unwrap();
            tracing::info!(%local_addr, "waiting for new connection");
            if let Some(incoming) = s.endpoint.accept().await {
                tracing::info!("accepted new connection");
                if let Ok(conn) = incoming.await {
                    let (s, r) = conn.accept_bi().await.unwrap();
                    let send = table.push_child(Box::new(AsyncWriteStream::new(8192, s)) as Box<dyn 'static + wasmtime_wasi::OutputStream>, &socket).unwrap();
                    let recv= table.push_child(Box::new(AsyncReadStream::new(r)) as Box<dyn 'static + wasmtime_wasi::InputStream>, &socket).unwrap();
                    return Ok((socket, recv, send));
                }
            }
            Err(ErrorCode::Unknown)
        }
    }

    fn subscribe(
        &mut self,
        socket: wasmtime::component::Resource<QuicSocket>,
    ) -> impl ::core::future::Future<Output = wasmtime::component::Resource<Pollable>> {
        async move { wasmtime_wasi_io::poll::subscribe(&mut *self.table.lock().await, socket).unwrap() }
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

        let table = self.table.clone();
        async move { 
            let table = table.lock().await;
            if let Ok(socket) = table.get(&socket) {
                return socket.endpoint.local_addr().map(sockaddr_to_wasi).map_err(|_| ErrorCode::InvalidState)
            }
            Err(ErrorCode::InvalidState)
        }
    }

    fn shutdown(
        &mut self,
        socket: wasmtime::component::Resource<QuicSocket>,
        _shutdown_type: ShutdownType,
    ) -> impl ::core::future::Future<Output = Result<(), ErrorCode>> + ::core::marker::Send {
         let table = self.table.clone();
        async move { 
            let mut table = table.lock().await;
            if let Ok(socket) = table.delete(socket) {
                socket.endpoint.close(VarInt::from_u32(0), b"server closed");
            }
            Ok(())
        }
    }

    fn drop(
        &mut self,
        rep: wasmtime::component::Resource<QuicSocket>,
    ) -> impl ::core::future::Future<Output = wasmtime::Result<()>> + ::core::marker::Send {
        let table = self.table.clone();
        async move { 
            let mut table = table.lock().await;
            let _ = table.delete(rep);
            Ok(())
        }
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
    
    
    fn configure_server()
        -> Result<(ServerConfig, CertificateDer<'static>), Box<dyn Error + Send + Sync + 'static>> {
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let cert_der = CertificateDer::from(cert.cert);
        let priv_key = PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der());
    
        let mut server_config =
            ServerConfig::with_single_cert(vec![cert_der.clone()], priv_key.into())?;
        let transport_config = Arc::get_mut(&mut server_config.transport).unwrap();
        transport_config.max_concurrent_uni_streams(0_u8.into());
    
        Ok((server_config, cert_der))
    }
}

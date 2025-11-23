use std::net::SocketAddr;

use wasip2::sockets::network::{IpSocketAddress, Ipv4SocketAddress};
use wstd::{
    io::{self, AsyncPollable},
    iter::AsyncIterator,
};

use crate::{
    quic_stream::QuicStream,
    rvm::lambda::quic::{QuicSocket, create_quic_socket},
    to_io_err,
};

#[derive(Debug)]
pub struct QuicListener {
    pollable: AsyncPollable,
    socket: QuicSocket,
}

impl QuicListener {
    pub async fn bind(addr: &str) -> io::Result<Self> {
        let addr: SocketAddr = addr
            .parse()
            .map_err(|_| io::Error::other("failed to parse string to socket addr"))?;
        let socket = create_quic_socket(sockaddr_to_wasi(addr)).await.map_err(to_io_err)?;
        let pollable = AsyncPollable::new(socket.subscribe().await);
        Ok(Self { pollable, socket })
    }

    #[allow(dead_code)]
    pub async fn local_addr(&self) -> io::Result<std::net::SocketAddr> {
        self.socket
            .local_address()
            .await
            .map_err(to_io_err)
            .map(sockaddr_from_wasi)
    }

    pub fn incoming(&self) -> Incoming<'_> {
        Incoming { listener: self }
    }
}

/// An iterator that infinitely accepts connections on a QuicListener.
#[derive(Debug)]
pub struct Incoming<'a> {
    listener: &'a QuicListener,
}

impl<'a> AsyncIterator for Incoming<'a> {
    type Item = io::Result<QuicStream>;

    async fn next(&mut self) -> Option<Self::Item> {
        self.listener.pollable.wait_for().await;
        match self.listener.socket.accept().await.map_err(to_io_err) {
            Ok((socket, input, output)) => {
                crate::rvm::lambda::host::log(String::from("accepted connection")).await;
                Some(Ok(QuicStream::new(input, output, socket)))
            }
            Err(err) => {
                crate::rvm::lambda::host::log(format!("unexpected failure {err}")).await;
                Some(Err(err))
            }
        }
    }
}

fn sockaddr_from_wasi(addr: IpSocketAddress) -> std::net::SocketAddr {
    use wasip2::sockets::network::Ipv6SocketAddress;
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

fn sockaddr_to_wasi(addr: std::net::SocketAddr) -> IpSocketAddress {
    use wasip2::sockets::network::Ipv6SocketAddress;
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

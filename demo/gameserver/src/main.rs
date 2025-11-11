mod quic;
mod quic_stream;

use std::io::ErrorKind;

use wit_bindgen::generate;
use wstd::io::{self, AsyncRead, AsyncWrite};
use wstd::iter::AsyncIterator;

use crate::rvm::lambda::quic::ErrorCode;

generate!({
    world: "rvm-quic",
    path: "../../wit",
    with: {
        "wasi:io/poll@0.2.3":  wstd::wasip2::io::poll,
        "wasi:io/error@0.2.3":  wstd::wasip2::io::error,
        "wasi:io/streams@0.2.3":  wstd::wasip2::io::streams,
        "wasi:clocks/monotonic-clock@0.2.3":  wstd::wasip2::clocks::monotonic_clock,
        "wasi:sockets/network@0.2.3":  wstd::wasip2::sockets::network,
    },
});

fn to_io_err(err: ErrorCode) -> io::Error {
    match err {
        ErrorCode::Unknown => ErrorKind::Other.into(),
        ErrorCode::AccessDenied => ErrorKind::PermissionDenied.into(),
        ErrorCode::NotSupported => ErrorKind::Unsupported.into(),
        ErrorCode::InvalidArgument => ErrorKind::InvalidInput.into(),
        ErrorCode::OutOfMemory => ErrorKind::OutOfMemory.into(),
        ErrorCode::Timeout => ErrorKind::TimedOut.into(),
        ErrorCode::WouldBlock => ErrorKind::WouldBlock.into(),
        ErrorCode::InvalidState => ErrorKind::InvalidData.into(),
        ErrorCode::AddressInUse => ErrorKind::AddrInUse.into(),
        ErrorCode::ConnectionRefused => ErrorKind::ConnectionRefused.into(),
        ErrorCode::ConnectionReset => ErrorKind::ConnectionReset.into(),
        ErrorCode::ConnectionAborted => ErrorKind::ConnectionAborted.into(),
        ErrorCode::ConcurrencyConflict => ErrorKind::AlreadyExists.into(),
        _ => ErrorKind::Other.into(),
    }
}

#[wstd::main]
async fn main() -> io::Result<()> {
    let listener = crate::quic::QuicListener::bind("127.0.0.1:8086").await?;
    let mut incoming = listener.incoming();
    while let Some(Ok(msg)) = incoming.next().await {
        if let Some(output) = msg.as_async_output_stream() {
            if let Some(input) = msg.as_async_input_stream() {
                let buf = &mut [];
                input.read(buf).await?;
                let _ = output.write(buf).await;
                let _ = output.flush().await;
            }
        }

    }
    Ok(())
}

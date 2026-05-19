use wasip2::io::streams::{InputStream, OutputStream};

use wstd::io::{self, AsyncInputStream, AsyncOutputStream};

/// A bidirectional QUIC stream.
pub struct QuicStream {
    input: AsyncInputStream,
    output: AsyncOutputStream,
}

impl QuicStream {
    pub(crate) fn new(input: InputStream, output: OutputStream) -> Self {
        QuicStream {
            input: AsyncInputStream::new(input),
            output: AsyncOutputStream::new(output),
        }
    }

    pub fn split(&self) -> (ReadHalf<'_>, WriteHalf<'_>) {
        (ReadHalf(self), WriteHalf(self))
    }
}

impl io::AsyncRead for QuicStream {
    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.input.read(buf).await
    }

    fn as_async_input_stream(&self) -> Option<&AsyncInputStream> {
        Some(&self.input)
    }
}

impl io::AsyncRead for &QuicStream {
    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.input.read(buf).await
    }

    fn as_async_input_stream(&self) -> Option<&AsyncInputStream> {
        (**self).as_async_input_stream()
    }
}

impl io::AsyncWrite for QuicStream {
    async fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.output.write(buf).await
    }

    async fn flush(&mut self) -> io::Result<()> {
        self.output.flush().await
    }

    fn as_async_output_stream(&self) -> Option<&AsyncOutputStream> {
        Some(&self.output)
    }
}

impl io::AsyncWrite for &QuicStream {
    async fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.output.write(buf).await
    }

    async fn flush(&mut self) -> io::Result<()> {
        self.output.flush().await
    }

    fn as_async_output_stream(&self) -> Option<&AsyncOutputStream> {
        (**self).as_async_output_stream()
    }
}

pub struct ReadHalf<'a>(&'a QuicStream);
impl<'a> io::AsyncRead for ReadHalf<'a> {
    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf).await
    }

    fn as_async_input_stream(&self) -> Option<&AsyncInputStream> {
        self.0.as_async_input_stream()
    }
}

pub struct WriteHalf<'a>(&'a QuicStream);
impl<'a> io::AsyncWrite for WriteHalf<'a> {
    async fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf).await
    }

    async fn flush(&mut self) -> io::Result<()> {
        self.0.flush().await
    }

    fn as_async_output_stream(&self) -> Option<&AsyncOutputStream> {
        self.0.as_async_output_stream()
    }
}

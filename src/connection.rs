//! Connection abstraction for plain and TLS streams

use tokio::net::TcpStream;
use tokio_native_tls::{TlsAcceptor, TlsStream};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter, ReadHalf, WriteHalf};
use tokio::sync::Mutex;

use crate::error::{Error, Result};
use crate::protocol::Response;

/// A connection that can be either plain TCP or TLS-wrapped
/// Uses split reader/writer with BufReader/BufWriter for efficiency
pub enum Connection {
    /// Plain TCP connection
    Plain {
        reader: Mutex<BufReader<ReadHalf<TcpStream>>>,
        writer: Mutex<BufWriter<WriteHalf<TcpStream>>>,
        peer_addr: std::net::SocketAddr,
    },
    /// TLS-encrypted connection
    Tls {
        reader: Mutex<BufReader<ReadHalf<TlsStream<TcpStream>>>>,
        writer: Mutex<BufWriter<WriteHalf<TlsStream<TcpStream>>>>,
        peer_addr: std::net::SocketAddr,
    },
}

impl Connection {
    /// Create a new plain TCP connection
    pub fn plain(stream: TcpStream) -> Result<Self> {
        let peer_addr = stream.peer_addr()?;
        let (read_half, write_half) = tokio::io::split(stream);
        Ok(Self::Plain {
            reader: Mutex::new(BufReader::new(read_half)),
            writer: Mutex::new(BufWriter::new(write_half)),
            peer_addr,
        })
    }

    /// Create a new TLS connection
    pub fn tls(stream: TlsStream<TcpStream>, peer_addr: std::net::SocketAddr) -> Self {
        let (read_half, write_half) = tokio::io::split(stream);
        Self::Tls {
            reader: Mutex::new(BufReader::new(read_half)),
            writer: Mutex::new(BufWriter::new(write_half)),
            peer_addr,
        }
    }

    /// Check if this connection is using TLS
    pub fn is_tls(&self) -> bool {
        matches!(self, Connection::Tls { .. })
    }

    /// Upgrade a plain connection to TLS
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The connection is already using TLS
    /// - The TLS handshake fails
    pub async fn upgrade_to_tls(self, acceptor: &TlsAcceptor) -> Result<Self> {
        match self {
            Connection::Plain { reader, writer, peer_addr } => {
                // Unsplit the read and write halves
                let read_half = reader.into_inner().into_inner();
                let write_half = writer.into_inner().into_inner();
                let tcp_stream = read_half.unsplit(write_half);

                // Perform TLS handshake
                let tls_stream = acceptor.accept(tcp_stream).await
                    .map_err(|e| Error::TlsError(format!("TLS handshake failed: {}", e)))?;

                // Create new connection with TLS stream
                Ok(Connection::tls(tls_stream, peer_addr))
            }
            Connection::Tls { .. } => {
                Err(Error::ProtocolError("Connection already using TLS".to_string()))
            }
        }
    }

    /// Read a line from the connection
    ///
    /// Reads until \n (removes trailing \r\n)
    pub async fn read_line(&self, buf: &mut String) -> Result<usize> {
        match self {
            Connection::Plain { reader, .. } => {
                let mut guard = reader.lock().await;
                let bytes = guard.read_line(buf).await?;
                Ok(bytes)
            }
            Connection::Tls { reader, .. } => {
                let mut guard = reader.lock().await;
                let bytes = guard.read_line(buf).await?;
                Ok(bytes)
            }
        }
    }

    /// Write a response to the connection
    pub async fn write_response(&self, response: &Response) -> Result<()> {
        let response_str = response.to_string();
        match self {
            Connection::Plain { writer, .. } => {
                let mut guard = writer.lock().await;
                guard.write_all(response_str.as_bytes()).await?;
                guard.flush().await?;
                Ok(())
            }
            Connection::Tls { writer, .. } => {
                let mut guard = writer.lock().await;
                guard.write_all(response_str.as_bytes()).await?;
                guard.flush().await?;
                Ok(())
            }
        }
    }

    /// Write raw bytes to the connection
    pub async fn write_all(&self, buf: &[u8]) -> Result<()> {
        match self {
            Connection::Plain { writer, .. } => {
                let mut guard = writer.lock().await;
                guard.write_all(buf).await?;
                guard.flush().await?;
                Ok(())
            }
            Connection::Tls { writer, .. } => {
                let mut guard = writer.lock().await;
                guard.write_all(buf).await?;
                guard.flush().await?;
                Ok(())
            }
        }
    }

    /// Get the peer address of the underlying TCP stream
    pub fn peer_addr(&self) -> Option<std::net::SocketAddr> {
        match self {
            Connection::Plain { peer_addr, .. } => Some(*peer_addr),
            Connection::Tls { peer_addr, .. } => Some(*peer_addr),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::{TcpListener, TcpStream};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn create_test_connection() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let client_task = tokio::spawn(async move {
            TcpStream::connect(addr).await.unwrap()
        });

        let (server_stream, _) = listener.accept().await.unwrap();
        let client_stream = client_task.await.unwrap();

        (server_stream, client_stream)
    }

    #[tokio::test]
    async fn test_plain_connection() {
        let (stream, _client) = create_test_connection().await;
        let conn = Connection::plain(stream).unwrap();

        assert!(!conn.is_tls());
    }

    #[tokio::test]
    async fn test_is_tls() {
        let (stream, _client) = create_test_connection().await;
        let plain_conn = Connection::plain(stream).unwrap();
        assert!(!plain_conn.is_tls());
    }

    #[tokio::test]
    async fn test_read_write_plain() {
        let (server_stream, mut client_stream) = create_test_connection().await;
        let conn = Connection::plain(server_stream).unwrap();

        // Write from client
        client_stream.write_all(b"Hello\n").await.unwrap();
        client_stream.flush().await.unwrap();

        // Read from server connection
        let mut buf = String::new();
        conn.read_line(&mut buf).await.unwrap();
        assert_eq!(buf.trim(), "Hello");
    }

    #[tokio::test]
    async fn test_write_response() {
        let (server_stream, mut client_stream) = create_test_connection().await;
        let conn = Connection::plain(server_stream).unwrap();

        // Write response from server
        let response = Response::Ok {
            tag: Some("A001".to_string()),
            message: "Test message".to_string(),
        };
        conn.write_response(&response).await.unwrap();

        // Read from client
        let mut buf = vec![0u8; 1024];
        let n = client_stream.read(&mut buf).await.unwrap();
        let received = String::from_utf8_lossy(&buf[..n]);
        assert!(received.contains("A001"));
        assert!(received.contains("Test message"));
    }
}

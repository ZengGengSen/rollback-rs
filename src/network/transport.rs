//! UDP transport layer: send/receive wrapper around tokio::net::UdpSocket

use std::net::SocketAddr;

use tokio::net::UdpSocket;
use tokio::sync::mpsc;

/// Maximum UDP packet size in bytes
pub const MAX_PACKET_SIZE: usize = 4096;

/// UDP transport layer
///
/// Internally holds an `Arc<UdpSocket>` so it can be safely shared across async tasks.
pub struct UdpTransport {
    socket: std::sync::Arc<UdpSocket>,
}

impl UdpTransport {
    /// Bind to a local address and create the transport layer
    pub async fn bind(addr: &str) -> std::io::Result<Self> {
        let socket = UdpSocket::bind(addr).await?;
        Ok(Self {
            socket: std::sync::Arc::new(socket),
        })
    }

    /// Send raw bytes to the target address
    pub async fn send_to(&self, data: &[u8], target: SocketAddr) -> std::io::Result<usize> {
        self.socket.send_to(data, target).await
    }

    /// Spawn a background receive task.
    ///
    /// Returns a channel receiver that yields `(source address, raw bytes)` for each packet received.
    pub fn spawn_recv_task(&self) -> mpsc::UnboundedReceiver<(SocketAddr, Vec<u8>)> {
        let (tx, rx) = mpsc::unbounded_channel();
        let socket = self.socket.clone();

        tokio::spawn(async move {
            let mut buf = vec![0u8; MAX_PACKET_SIZE];
            loop {
                match socket.recv_from(&mut buf).await {
                    Ok((n, addr)) => {
                        let _ = tx.send((addr, buf[..n].to_vec()));
                    }
                    Err(e) => {
                        eprintln!("[transport] recv error: {e}");
                        break;
                    }
                }
            }
        });

        rx
    }

    /// Returns the local bound address
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.socket.local_addr()
    }
}

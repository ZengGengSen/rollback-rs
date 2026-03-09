//! Network Session: integrates UdpTransport, PeerState, and RollbackSession
//!
//! Provides a clean high-level interface:
//! - `advance_frame`: step one frame (including network send/receive)
//! - `poll`: process all pending incoming packets (non-blocking)
//! - `send_input`: serialize the local input and send it to all peers

use std::net::SocketAddr;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use super::packet::{NetworkPacket, PacketKind};
use super::peer::PeerState;
use super::transport::UdpTransport;
use crate::error::RollbackError;
use crate::state::RollbackState;
use crate::sync::RollbackSession;

/// Network Session
///
/// Generic parameter `S` is the game state type, which must implement `RollbackState`.
/// `S::Input` must also implement `Serialize + Deserialize` (already implied by the RollbackState bound).
pub struct NetworkSession<S: RollbackState> {
    /// Underlying rollback engine
    pub rollback: RollbackSession<S>,

    /// UDP transport layer
    transport: UdpTransport,

    /// State for each remote peer
    peers: Vec<PeerState>,

    /// Local player ID
    local_player_id: usize,

    /// Channel receiver for incoming packets
    recv_rx: mpsc::UnboundedReceiver<(SocketAddr, Vec<u8>)>,

    /// Maximum rollback frames (used for stall detection)
    max_rollback_frames: u32,
}

impl<S: RollbackState> NetworkSession<S>
where
    S::Input: Serialize + for<'a> Deserialize<'a>,
{
    /// Bind a local address and create a network session
    pub async fn bind(
        local_addr: &str,
        local_player_id: usize,
        initial_state: S,
        player_count: usize,
        max_rollback: u32,
        max_input_delay: u32,
        input_delay: u32,
    ) -> std::io::Result<Self> {
        let transport = UdpTransport::bind(local_addr).await?;
        let recv_rx = transport.spawn_recv_task();
        let rollback = RollbackSession::with_input_delay(
            initial_state,
            player_count,
            max_rollback,
            max_input_delay,
            input_delay,
        );

        Ok(Self {
            rollback,
            transport,
            peers: Vec::new(),
            local_player_id,
            recv_rx,
            max_rollback_frames: max_rollback,
        })
    }

    pub fn local_player_id(&self) -> usize {
        self.local_player_id
    }

    /// Register a remote peer
    pub fn add_peer(&mut self, addr: SocketAddr, player_id: usize) {
        self.peers.push(PeerState::new(addr, player_id));
    }

    /// Returns the local bound address
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.transport.local_addr()
    }

    // -------------------------------------------------------------------------
    // Game main loop interface
    // -------------------------------------------------------------------------

    /// Advance one frame:
    /// 1. Process all pending incoming packets
    /// 2. Check whether a stall is needed (waiting for the remote peer to catch up)
    /// 3. Call rollback.advance_frame
    /// 4. Send the local input to all peers
    ///
    /// Returns `Ok(true)` if the frame was advanced normally, `Ok(false)` if the frame was stalled (simulation skipped)
    pub async fn advance_frame(&mut self, local_input: S::Input) -> Result<bool, RollbackError> {
        // 1. Process all pending packets
        self.poll().await;

        // 2. Stall detection: wait if local simulation is too far ahead of the remote peer
        let local_frame = self.rollback.current_frame();
        let should_stall = self
            .peers
            .iter()
            .any(|p| p.should_stall(local_frame, self.max_rollback_frames));

        if should_stall {
            return Ok(false);
        }

        // 3. Send local input to all peers (with redundancy)
        self.send_input(&local_input).await;

        // 4. Step the rollback session
        self.rollback
            .advance_frame(self.local_player_id, local_input)?;

        Ok(true)
    }

    /// Process all currently available incoming packets (non-blocking, drains the channel)
    pub async fn poll(&mut self) {
        // Drain all immediately available packets without blocking for new ones
        loop {
            match self.recv_rx.try_recv() {
                Ok((addr, data)) => {
                    self.handle_packet(addr, &data).await;
                }
                Err(_) => break,
            }
        }
    }

    /// Send the local input to all peers (with redundant frames)
    pub async fn send_input(&mut self, input: &S::Input) {
        let current_frame = self.rollback.current_frame();
        let input_bytes = match bincode::serialize(input) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("[net] failed to serialize input: {e}");
                return;
            }
        };

        for peer in &mut self.peers {
            // Record the current input into history (used to build redundant packets)
            peer.record_sent_input(current_frame, input_bytes.clone());

            let (start_frame, redundant_bytes) = peer.get_redundant_inputs(current_frame);

            // Deserialize Vec<Vec<u8>> back into Vec<S::Input>
            let mut inputs: Vec<S::Input> = Vec::new();
            let mut valid = true;
            for b in &redundant_bytes {
                match bincode::deserialize::<S::Input>(b) {
                    Ok(inp) => inputs.push(inp),
                    Err(e) => {
                        eprintln!("[net] failed to deserialize redundant input: {e}");
                        valid = false;
                        break;
                    }
                }
            }
            if !valid {
                continue;
            }

            let packet = NetworkPacket {
                sender_frame: current_frame,
                kind: PacketKind::Input {
                    player_id: self.local_player_id,
                    start_frame,
                    inputs,
                },
            };

            if let Ok(data) = packet.serialize() {
                let _ = self.transport.send_to(&data, peer.addr).await;
            }
        }
    }

    /// Send a Ping to all peers
    pub async fn send_ping(&mut self) {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        for peer in &mut self.peers {
            peer.on_ping_sent();
            let packet: NetworkPacket<S::Input> = NetworkPacket {
                sender_frame: self.rollback.current_frame(),
                kind: PacketKind::Ping {
                    timestamp_ms: now_ms,
                },
            };
            if let Ok(data) = packet.serialize() {
                let _ = self.transport.send_to(&data, peer.addr).await;
            }
        }
    }

    /// Send the confirmed checksum to all peers
    pub async fn send_checksum(&mut self) {
        let frame = self.rollback.confirmed_frame();
        let checksum = self.rollback.confirmed_state().checksum();

        for peer in &self.peers {
            let packet: NetworkPacket<S::Input> = NetworkPacket {
                sender_frame: self.rollback.current_frame(),
                kind: PacketKind::Checksum { frame, checksum },
            };
            if let Ok(data) = packet.serialize() {
                let _ = self.transport.send_to(&data, peer.addr).await;
            }
        }
    }

    // -------------------------------------------------------------------------
    // Internal packet handling
    // -------------------------------------------------------------------------

    async fn handle_packet(&mut self, from: SocketAddr, data: &[u8]) {
        let packet = match NetworkPacket::<S::Input>::deserialize(data) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("[net] failed to deserialize packet from {from}: {e}");
                return;
            }
        };

        // Look up the peer this packet came from
        let peer_idx = self.peers.iter().position(|p| p.addr == from);

        match packet.kind {
            PacketKind::Input {
                player_id,
                start_frame,
                inputs,
            } => {
                // Update the peer's remote_confirmed_frame
                if let Some(idx) = peer_idx {
                    self.peers[idx].update_remote_confirmed(packet.sender_frame);
                }

                // Submit the batch to the rollback session
                if let Err(e) = self
                    .rollback
                    .add_remote_inputs(player_id, start_frame, inputs)
                {
                    // InputTooOld is expected for redundant packets; handle silently
                    match e {
                        RollbackError::InputTooOld { .. } => {}
                        other => eprintln!("[net] add_remote_inputs failed: {other}"),
                    }
                }
            }

            PacketKind::Ping { timestamp_ms } => {
                // Reply with a Pong
                if let Some(idx) = peer_idx {
                    let pong: NetworkPacket<S::Input> = NetworkPacket {
                        sender_frame: self.rollback.current_frame(),
                        kind: PacketKind::Pong {
                            echo_timestamp_ms: timestamp_ms,
                        },
                    };
                    if let Ok(data) = pong.serialize() {
                        let _ = self.transport.send_to(&data, self.peers[idx].addr).await;
                    }
                }
            }

            PacketKind::Pong { .. } => {
                // Update RTT
                if let Some(idx) = peer_idx {
                    self.peers[idx].on_pong_received();
                    let delay = self.peers[idx].suggested_input_delay();
                    let rtt = self.peers[idx].rtt();
                    println!(
                        "[net] peer {} RTT={:.1}ms, suggested input_delay={}",
                        from,
                        rtt.as_millis(),
                        delay
                    );
                }
            }

            PacketKind::Checksum { frame, checksum } => {
                // Out-of-sync detection
                if let Err(e) = self.rollback.verify_checksum(frame, checksum) {
                    eprintln!("[net] ⚠️  OOS detected from {from}: {e}");
                }
            }
        }
    }
}

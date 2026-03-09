//! Network packet definitions
//!
//! Each UDP packet carries:
//! - The sender's current frame number
//! - A contiguous sequence of inputs starting from a given frame (redundantly sent to combat packet
//!   loss)
//! - The sender's confirmed_frame + checksum (used for out-of-sync detection)

use serde::{Deserialize, Serialize};

/// Network packet type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PacketKind<I> {
    /// Input packet: carries multiple frames of input starting from start_frame
    Input {
        /// Local player ID
        player_id: usize,
        /// Frame number of the first input in the batch
        start_frame: u32,
        /// Contiguous inputs starting from start_frame (includes redundant frames)
        inputs: Vec<I>,
    },

    /// Heartbeat / Ping packet: used for RTT measurement
    Ping {
        /// Local timestamp at send time (milliseconds)
        timestamp_ms: u64,
    },

    /// Pong packet: reply to a Ping
    Pong {
        /// Timestamp echoed from the original Ping
        echo_timestamp_ms: u64,
    },

    /// Checksum sync packet
    Checksum {
        /// The confirmed_frame this checksum corresponds to
        frame: u32,
        /// Checksum of that frame's state
        checksum: u64,
    },
}

/// Complete network packet (includes the sender's current frame number)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkPacket<I> {
    /// The frame the sender has simulated up to (used to estimate the peer's latency)
    pub sender_frame: u32,
    /// Packet payload
    pub kind: PacketKind<I>,
}

impl<I: Serialize + for<'a> Deserialize<'a>> NetworkPacket<I> {
    pub fn serialize(&self) -> Result<Vec<u8>, bincode::Error> {
        bincode::serialize(self)
    }

    pub fn deserialize(bytes: &[u8]) -> Result<Self, bincode::Error> {
        bincode::deserialize(bytes)
    }
}

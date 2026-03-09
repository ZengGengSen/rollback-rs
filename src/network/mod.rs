//! Network layer: UDP + tokio based rollback network transport
//!
//! Usage:
//! 1. Bind a local port with `NetworkSession::bind`
//! 2. Register remote addresses with `add_peer`
//! 3. Call `advance_frame` in the game main loop to step the simulation
//! 4. A background task automatically receives packets, triggers rollbacks, and updates confirmed state

pub mod packet;
pub mod peer;
pub mod session;
pub mod transport;

pub use packet::{NetworkPacket, PacketKind};
pub use peer::PeerState;
pub use session::NetworkSession;
pub use transport::UdpTransport;

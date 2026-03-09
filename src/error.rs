use thiserror::Error;

/// Error type for rollback failures.
///
/// Each error variant documents a specific rollback failure mode.
#[derive(Debug, Error)]
pub enum RollbackError {
    /// The received input frame is too old and has exceeded the rollback window.
    #[error(
        "Input frame {frame} is older than the confirmed frame {confirmed_frame}, cannot be processed"
    )]
    InputTooOld { frame: u32, confirmed_frame: u32 },

    /// The player ID is out of bounds.
    #[error("Player ID {player_id} is out of range, current player count is {player_count}")]
    InvalidPlayerId {
        player_id: usize,
        player_count: usize,
    },

    /// The number of frames to roll back exceeds the maximum allowed value.
    #[error("Rollback of {needed} frames requested, but maximum allowed is {max} frames")]
    RollbackTooFar { needed: u32, max: u32 },

    /// Snapshot not found (typically indicates internal logic error).
    #[error("Snapshot for frame {frame} not found")]
    SnapshotNotFound { frame: u32 },

    /// Out-of-sync: checksum mismatch detected between local and remote peers.
    #[error(
        "Out-of-Sync detected at frame {frame}: local checksum {local}, remote checksum {remote}"
    )]
    OutOfSync { frame: u32, local: u64, remote: u64 },
}

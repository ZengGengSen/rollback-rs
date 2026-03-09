use std::hash::Hasher;

use ahash::AHasher;
use serde::{Deserialize, Serialize};

/// Every game state must be able to be saved and restored.
pub trait RollbackState: Clone + Serialize + for<'a> Deserialize<'a> {
    /// Defines the input type for this state.
    type Input: Clone + Serialize + for<'a> Deserialize<'a> + PartialEq + Default;

    /// Core logic: advance the state by one frame according to input.
    fn advance(&mut self, inputs: &[Self::Input]);

    /// Calculates a checksum for detecting OOS (Out of Sync).
    /// Default implementation: serializes using bincode, then hashes using ahash.
    fn checksum(&self) -> u64 {
        let bytes = bincode::serialize(self).unwrap_or_default();
        let mut hasher = AHasher::default();
        hasher.write(&bytes);
        hasher.finish()
    }
}

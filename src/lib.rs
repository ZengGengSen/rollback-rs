//! rollback-rs
//!
//! Crate-level documentation and API examples.
//!
//! This file contains example doc comments for public API items. Replace or extend
//! these with concrete documentation for your types and functions.
pub mod error;
pub mod state;
pub mod sync;

#[cfg(feature = "network")]
pub mod network;

#[cfg(test)]
mod tests;

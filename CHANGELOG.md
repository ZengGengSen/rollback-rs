# Changelog

All notable changes to this project will be documented in this file.

## [0.1.0] - 2026-03-09

### Added

- **Core Rollback Engine**
  - Introduced `RollbackState` trait with support for serialization and checksum calculation via `bincode` and `ahash`.
  - Implemented `RollbackSession` featuring snapshot/restore capabilities and "repeat-last-frame" input prediction.
  - Added input delay queues and a force-confirmation mechanism to ensure deterministic synchronization.
  - Supported batch remote input processing with single-pass rollback re-simulation.
  - Defined `RollbackError` types including `InvalidPlayerId`, `InputTooOld`, `RollbackTooFar`, and `OutOfSync`.

- **Networking (Feature: `network`)**
  - Implemented an asynchronous UDP transport layer using `tokio` with background receive tasks.
  - Developed `NetworkSession` to integrate transport, peer management, and the rollback engine.
  - Added RTT tracking using EWMA (Exponential Weighted Moving Average) and frame advantage calculations.
  - Implemented adaptive redundancy windows and stall detection to optimize performance under unstable network conditions.
  - Defined `NetworkPacket` protocols for `Input`, `Ping`, `Pong`, and `Checksum` data.

- **Testing & Examples**
  - Added 27 unit tests covering state progression, checksum validation, rollback correctness, and OOS detection.
  - Created a `p2p_demo` example demonstrating a complete peer-to-peer rollback workflow between two players.

- **Tooling & Configuration**
  - Configured `rustfmt.toml` (nightly) for optimized import grouping and code alignment.
  - Added project-specific settings for the Zed editor and environment configurations.

### Changed

- Initialized the project architecture and encapsulated network-related logic under the `network` feature flag (enabled by default).

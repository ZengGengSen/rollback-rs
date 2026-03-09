# rollback-rs

A deterministic rollback netcode library for real-time multiplayer games, built in Rust.

## Overview

`rollback-rs` implements the **rollback + prediction** model used in fighting games and other latency-sensitive multiplayer titles. Rather than waiting for remote inputs before simulating, each peer predicts missing inputs and speculatively advances. When the real input arrives, the engine rolls back to the divergence point and re-simulates forward — all within a single frame budget.

The library is split into two layers:

| Crate module                | Responsibility                                                 |
| --------------------------- | -------------------------------------------------------------- |
| `sync`                      | Core rollback engine — snapshots, prediction, re-simulation    |
| `network` _(feature-gated)_ | UDP transport + peer management + `NetworkSession` integration |

---

## Features

- **Rollback & re-simulation** — configurable window (e.g. 8 frames)
- **Input delay** — defer local input N frames to reduce rollback frequency
- **Redundant input delivery** — each UDP packet carries the last N frames of input to tolerate packet loss
- **Force-confirmation** — prevents unbounded input history growth when a remote peer goes silent
- **Out-of-sync (OOS) detection** — checksum comparison via `verify_checksum`
- **RTT-based stall** — local simulation pauses when it races too far ahead of a peer
- **Adaptive redundancy** — redundancy window auto-adjusts based on measured RTT

---

## Architecture

```
┌─────────────────────────────────────────┐
│              Your Game Loop             │
└────────────────┬────────────────────────┘
                 │ advance_frame(local_input)
┌────────────────▼────────────────────────┐
│           NetworkSession<S>             │  (network feature)
│  poll() → stall check → send_input()    │
└────────────────┬────────────────────────┘
                 │
┌────────────────▼────────────────────────┐
│          RollbackSession<S>             │
│  advance_frame / add_remote_input(s)    │
│  rollback_to → re-simulate              │
│  update_confirmed_state                 │
└────────────────┬────────────────────────┘
                 │
┌────────────────▼────────────────────────┐
│         S: RollbackState                │
│  advance(&[Input]) / checksum()         │
└─────────────────────────────────────────┘
```

---

## Quick Start

### 1. Implement `RollbackState`

```rust
use rollback_rs::state::RollbackState;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
struct GameState {
    positions: Vec<f32>,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Default)]
struct Input {
    dx: f32,
}

impl RollbackState for GameState {
    type Input = Input;

    fn advance(&mut self, inputs: &[Self::Input]) {
        for (pos, input) in self.positions.iter_mut().zip(inputs) {
            *pos += input.dx;
        }
    }
}
```

### 2. Create a session (local-only / headless)

```rust
use rollback_rs::sync::RollbackSession;

let mut session = RollbackSession::new(
    GameState { positions: vec![0.0, 0.0] },
    2,   // player_count
    8,   // max_rollback_frames
    16,  // max_input_delay (force-confirm threshold)
);

// Game loop
session.advance_frame(0, Input { dx: 1.0 })?;

// Remote input arrives (possibly out of order)
session.add_remote_input(1, 0, Input { dx: -1.0 })?;
```

### 3. With input delay

```rust
let mut session = RollbackSession::with_input_delay(
    initial_state,
    2,   // player_count
    8,   // max_rollback_frames
    16,  // max_input_delay
    2,   // input_delay: local input applied 2 frames later
);
```

Input delay trades a small fixed latency increase for a significant reduction in rollback frequency, since remote inputs have more time to arrive before they are needed.

### 4. Networked session

```rust
use rollback_rs::network::NetworkSession;

let mut net = NetworkSession::bind(
    "0.0.0.0:7000",
    0,              // local_player_id
    initial_state,
    2,              // player_count
    8,              // max_rollback_frames
    16,             // max_input_delay
    2,              // input_delay
).await?;

net.add_peer("192.168.1.2:7000".parse()?, 1);

// Game loop (async)
loop {
    let advanced = net.advance_frame(Input { dx: 1.0 }).await?;
    if !advanced {
        // Stalled — remote peer is too far behind; skip simulation this tick
    }

    // Periodically
    net.send_ping().await;
    net.send_checksum().await;
}
```

---

## Core API Reference

### `RollbackSession<S>`

| Method                                                             | Description                                                  |
| ------------------------------------------------------------------ | ------------------------------------------------------------ |
| `new(state, players, max_rollback, max_delay)`                     | Create a session with `input_delay = 0`                      |
| `with_input_delay(state, players, max_rollback, max_delay, delay)` | Create a session with input delay                            |
| `advance_frame(player_id, input)`                                  | Step the simulation one frame                                |
| `add_remote_input(player_id, frame, input)`                        | Submit a single confirmed remote input                       |
| `add_remote_inputs(player_id, start_frame, inputs)`                | Submit a batch (handles redundant delivery, single rollback) |
| `verify_checksum(frame, remote_checksum)`                          | OOS detection against confirmed state                        |
| `current_state()`                                                  | Current (possibly predicted) state                           |
| `confirmed_state()`                                                | Latest fully confirmed state                                 |
| `current_frame()`                                                  | Current simulated frame number                               |
| `confirmed_frame()`                                                | Frame number of the confirmed state                          |
| `pending_frames()`                                                 | Frames currently in the input history queue                  |

### `NetworkSession<S>`

| Method                              | Description                                                                |
| ----------------------------------- | -------------------------------------------------------------------------- |
| `bind(addr, player_id, state, ...)` | Bind UDP socket and create session                                         |
| `add_peer(addr, player_id)`         | Register a remote peer                                                     |
| `advance_frame(input)`              | Poll network + stall check + simulate + send; returns `Ok(false)` on stall |
| `poll()`                            | Process all pending incoming packets (non-blocking)                        |
| `send_input(input)`                 | Serialize and send local input to all peers (with redundant frames)        |
| `send_ping()`                       | Send a Ping to all peers for RTT measurement                               |
| `send_checksum()`                   | Send confirmed-state checksum to all peers for OOS detection               |

### Error Types

| Variant                           | Cause                                                |
| --------------------------------- | ---------------------------------------------------- |
| `RollbackError::InvalidPlayerId`  | `player_id >= player_count`                          |
| `RollbackError::InputTooOld`      | `frame < confirmed_frame` (stale input)              |
| `RollbackError::RollbackTooFar`   | Rollback depth exceeds `max_rollback_frames`         |
| `RollbackError::SnapshotNotFound` | Internal: snapshot missing for the target frame      |
| `RollbackError::OutOfSync`        | Remote checksum does not match local confirmed state |

---

## Design Notes

### Prediction strategy

Missing remote inputs are filled with the **last known input** for that player (repeat-last-frame). `Default::default()` is used before any input has been received. This is identical to the strategy used by GGPO.

### Confirmed state advancement

The confirmed state advances frame-by-frame whenever all players' inputs for the oldest pending frame are known. If a frame stays unconfirmed longer than `max_input_delay` frames, it is **force-confirmed** using whatever inputs are currently available. This bounds the growth of the input history queue and prevents simulation stalls when a peer goes silent.

### Redundant input delivery

`NetworkSession::send_input` automatically bundles the most recent N frames of input into each packet, where N is derived from the measured RTT:

```
redundancy = ceil(RTT_ms / frame_duration_ms) + 2   (clamped to [2, 16])
```

On the receiving side, `add_remote_inputs` applies all frames in a single pass and performs at most **one rollback** (from the earliest divergence point), regardless of how many frames were wrong.

### Stall (frame advantage control)

If the local simulation is more than `max_rollback_frames` frames ahead of a peer's last confirmed frame, `advance_frame` returns `Ok(false)` and skips simulation that tick. This prevents the rollback window from being exceeded.

### Input delay

Setting `input_delay = N` schedules local input to be applied at `current_frame + N` rather than the current frame. Until then, the local player's slot is filled by prediction (same as any remote player). A delay of 2–4 frames is typical for LAN/WAN play and can reduce rollback occurrences to near zero at sub-50 ms RTT.

---

## Cargo Features

| Feature   | Default | Description                                                |
| --------- | ------- | ---------------------------------------------------------- |
| `network` | **yes** | `NetworkSession`、`UdpTransport`、`PeerState`、packet 类型 |

Enable in `Cargo.toml`:

```toml
[dependencies]
rollback-rs = { version = "0.1", features = ["network"] }
```

---

## Toolchain

The project uses nightly Rust for `rustfmt` features. A `rust-toolchain.toml` and `rustfmt.toml` are included. Format with:

```bash
cargo +nightly fmt
cargo clippy
cargo test
```

---

## License

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

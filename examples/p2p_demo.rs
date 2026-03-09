//! End-to-end P2P demo
//!
//! Spawns two tasks within the same process, simulating Player 0 and Player 1 respectively.
//! They communicate over local UDP to demonstrate the complete rollback netcode flow.

use std::net::SocketAddr;
use std::time::Duration;

use rollback_rs::network::NetworkSession;
use rollback_rs::state::RollbackState;
use serde::{Deserialize, Serialize};
use tokio::time::sleep;

// ---------------------------------------------------------------------------
// Game state definitions
// ---------------------------------------------------------------------------

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
struct GameState {
    positions: Vec<i32>,
}

#[derive(Clone, Serialize, Deserialize, Debug, Default, PartialEq, Copy)]
struct GameInput {
    delta: i32,
}

impl RollbackState for GameState {
    type Input = GameInput;

    fn advance(&mut self, inputs: &[Self::Input]) {
        for (pos, input) in self.positions.iter_mut().zip(inputs.iter()) {
            *pos += input.delta;
        }
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    println!("=== rollback-rs P2P Demo ===\n");

    let total_frames = 60u32;

    let handle_p0 = tokio::spawn(run_player(
        0,
        "127.0.0.1:19000",
        "127.0.0.1:19001".parse().unwrap(),
        total_frames,
    ));

    let handle_p1 = tokio::spawn(run_player(
        1,
        "127.0.0.1:19001",
        "127.0.0.1:19000".parse().unwrap(),
        total_frames,
    ));

    let (r0, r1) = tokio::join!(handle_p0, handle_p1);
    let final_state_p0 = r0.expect("P0 task panicked");
    let final_state_p1 = r1.expect("P1 task panicked");

    println!("\n=== Final state verification ===");
    println!("P0 confirmed_state: {:?}", final_state_p0);
    println!("P1 confirmed_state: {:?}", final_state_p1);

    if final_state_p0 == final_state_p1 {
        println!("\n✅ States are consistent — no OOS!");
    } else {
        println!("\n❌ State mismatch!");
    }
}

async fn run_player(
    player_id: usize,
    local_addr: &str,
    peer_addr: SocketAddr,
    total_frames: u32,
) -> GameState {
    let initial_state = GameState {
        positions: vec![0, 0],
    };

    let mut session = NetworkSession::bind(local_addr, player_id, initial_state, 2, 8, 24, 0)
        .await
        .expect("bind failed");

    session.add_peer(peer_addr, 1 - player_id);
    println!("[P{player_id}] listening on {local_addr} → peer {peer_addr}");

    sleep(Duration::from_millis(100)).await;
    session.send_ping().await;

    let frame_duration = Duration::from_millis(16);
    let mut simulated_frames = 0u32;
    let mut last_input = GameInput::default();

    while simulated_frames < total_frames {
        let loop_start = tokio::time::Instant::now();

        let delta = if player_id == 0 { 1 } else { 2 };
        let input = GameInput { delta };
        last_input = input;

        match session.advance_frame(input).await {
            Ok(true) => {
                simulated_frames += 1;
                let state = session.rollback.current_state();
                if simulated_frames.is_multiple_of(10) {
                    println!(
                        "[P{player_id}] frame {:>3} | positions: {:?} | confirmed_frame: {}",
                        simulated_frames,
                        state.positions,
                        session.rollback.confirmed_frame()
                    );
                }

                // Send a checksum every 30 frames
                if simulated_frames.is_multiple_of(30) {
                    session.send_checksum().await;
                }
            }
            Ok(false) => {
                // Stall: waiting for the remote peer to catch up; frame not counted
                println!("[P{player_id}] stall (waiting for peer)");
            }
            Err(e) => {
                eprintln!("[P{player_id}] failed to advance frame: {e}");
            }
        }

        let elapsed = loop_start.elapsed();
        if elapsed < frame_duration {
            sleep(frame_duration - elapsed).await;
        }
    }

    // --- Fix 1: proactively resend the last frame several times before waiting for confirmation,
    //            to ensure the packet reaches the peer ---
    for _ in 0..5 {
        session.send_input(&last_input).await;
        sleep(Duration::from_millis(10)).await;
    }

    wait_for_confirmed(&mut session, total_frames, last_input).await;

    println!(
        "[P{player_id}] done! confirmed_frame={}, positions={:?}",
        session.rollback.confirmed_frame(),
        session.rollback.confirmed_state().positions
    );

    session.rollback.confirmed_state().clone()
}

/// Confirmation wait logic: while waiting for the confirmed frame to reach the target,
/// resend the last input every 100 ms so the peer can receive the final frame and reach consensus.
async fn wait_for_confirmed(
    session: &mut NetworkSession<GameState>,
    target: u32,
    last_input: GameInput,
) {
    let player_id = if session.rollback.player_count() > 0 {
        // Infer local ID from the bound port for cleaner log output
        if session.local_addr().unwrap().port() == 19000 {
            0
        } else {
            1
        }
    } else {
        0
    };

    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let mut last_send = tokio::time::Instant::now();

    while session.rollback.confirmed_frame() < target {
        if tokio::time::Instant::now() >= deadline {
            eprintln!(
                "[P{player_id}] wait_for_confirmed timed out: confirmed_frame={} < target={}",
                session.rollback.confirmed_frame(),
                target
            );
            break;
        }

        // Periodically resend the last input packet (with redundant data)
        if last_send.elapsed() > Duration::from_millis(100) {
            session.send_input(&last_input).await;
            last_send = tokio::time::Instant::now();
        }

        session.poll().await;
        sleep(Duration::from_millis(10)).await;
    }
}

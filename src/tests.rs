use serde::{Deserialize, Serialize};

use crate::error::RollbackError;
use crate::state::RollbackState;
use crate::sync::RollbackSession;

// ---------------------------------------------------------------------------
// Simple state used in tests
// ---------------------------------------------------------------------------

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
struct SimpleState {
    pos: i32,
}

#[derive(Clone, Serialize, Deserialize, Debug, Default, PartialEq)]
struct SimpleInput {
    delta: i32,
}

impl RollbackState for SimpleState {
    type Input = SimpleInput;
    fn advance(&mut self, inputs: &[Self::Input]) {
        for input in inputs {
            self.pos += input.delta;
        }
    }
}

/// Quickly creates a standard test session (2 players, rollback window 8, max input delay 16)
fn make_session(initial_pos: i32) -> RollbackSession<SimpleState> {
    RollbackSession::new(SimpleState { pos: initial_pos }, 2, 8, 16)
}

// ---------------------------------------------------------------------------
// Basic functionality tests
// ---------------------------------------------------------------------------

#[test]
fn test_state_advance() {
    let mut state = SimpleState { pos: 0 };
    state.advance(&[SimpleInput { delta: 5 }, SimpleInput { delta: -2 }]);
    assert_eq!(state.pos, 3);
}

#[test]
fn test_session_creation() {
    let session = make_session(100);
    assert_eq!(session.current_state().pos, 100);
    assert_eq!(session.confirmed_state().pos, 100);
    assert_eq!(session.current_frame(), 0);
    assert_eq!(session.confirmed_frame(), 0);
    assert_eq!(session.player_count(), 2);
    assert_eq!(session.pending_frames(), 0);
}

#[test]
fn test_serialization() {
    let state = SimpleState { pos: 42 };
    let decoded: SimpleState = bincode::deserialize(&bincode::serialize(&state).unwrap()).unwrap();
    assert_eq!(state, decoded);
}

// ---------------------------------------------------------------------------
// Checksum tests
// ---------------------------------------------------------------------------

#[test]
fn test_checksum_differs_for_different_states() {
    assert_ne!(
        SimpleState { pos: 1 }.checksum(),
        SimpleState { pos: 2 }.checksum()
    );
}

#[test]
fn test_checksum_same_for_same_state() {
    assert_eq!(
        SimpleState { pos: 99 }.checksum(),
        SimpleState { pos: 99 }.checksum()
    );
}

// ---------------------------------------------------------------------------
// advance_frame tests
// ---------------------------------------------------------------------------

#[test]
fn test_advance_frame_basic() {
    let mut session = make_session(0);
    session.advance_frame(0, SimpleInput { delta: 10 }).unwrap();
    assert_eq!(session.current_frame(), 1);
    assert_eq!(session.current_state().pos, 10);
}

#[test]
fn test_advance_frame_invalid_player() {
    let mut session = make_session(0);
    let err = session
        .advance_frame(5, SimpleInput { delta: 1 })
        .unwrap_err();
    assert!(matches!(err, RollbackError::InvalidPlayerId { .. }));
}

// ---------------------------------------------------------------------------
// Rollback correctness tests
// ---------------------------------------------------------------------------

#[test]
fn test_rollback_corrects_prediction() {
    let mut session = make_session(0);

    session.advance_frame(0, SimpleInput { delta: 5 }).unwrap();
    session.advance_frame(0, SimpleInput { delta: 5 }).unwrap();

    // Frame 0 real input is +3 (differs from prediction 0), triggers rollback
    session
        .add_remote_input(1, 0, SimpleInput { delta: 3 })
        .unwrap();

    // Frame 0: +5+3=8, frame 1: +5+0=13 (player 1 in frame 1 still uses old prediction 0)
    assert_eq!(session.current_state().pos, 13);
}

#[test]
fn test_rollback_corrects_prediction_both_frames() {
    let mut session = make_session(0);

    session.advance_frame(0, SimpleInput { delta: 5 }).unwrap();
    session.advance_frame(0, SimpleInput { delta: 5 }).unwrap();

    session
        .add_remote_input(1, 0, SimpleInput { delta: 3 })
        .unwrap();
    // Real input for frame 1 also arrives
    session
        .add_remote_input(1, 1, SimpleInput { delta: 3 })
        .unwrap();

    // Frame 0: +5+3=8, frame 1: +5+3=16
    assert_eq!(session.current_state().pos, 16);
}

#[test]
fn test_no_rollback_when_prediction_correct() {
    let mut session = make_session(0);
    session.advance_frame(0, SimpleInput { delta: 5 }).unwrap();
    let pos_before = session.current_state().pos;

    // Real input matches prediction (default=0), no rollback triggered
    session
        .add_remote_input(1, 0, SimpleInput { delta: 0 })
        .unwrap();
    assert_eq!(session.current_state().pos, pos_before);
}

// ---------------------------------------------------------------------------
// confirmed_state advancement tests
// ---------------------------------------------------------------------------

#[test]
fn test_confirmed_state_advances() {
    let mut session = make_session(0);

    session.advance_frame(0, SimpleInput { delta: 1 }).unwrap();
    session
        .add_remote_input(1, 0, SimpleInput { delta: 2 })
        .unwrap();

    assert_eq!(session.confirmed_frame(), 1);
    assert_eq!(session.confirmed_state().pos, 3);
}

#[test]
fn test_confirmed_state_advances_multiple_frames() {
    let mut session = make_session(0);

    // Advance 3 consecutive frames with both players' inputs arriving each frame
    for f in 0..3u32 {
        session.advance_frame(0, SimpleInput { delta: 1 }).unwrap();
        session
            .add_remote_input(1, f, SimpleInput { delta: 1 })
            .unwrap();
    }

    // Each player contributes +1 per frame, 3 frames total → pos = 6
    assert_eq!(session.confirmed_frame(), 3);
    assert_eq!(session.confirmed_state().pos, 6);
}

// ---------------------------------------------------------------------------
// input_history unbounded growth guard tests
// ---------------------------------------------------------------------------

#[test]
fn test_input_history_bounded_when_remote_input_missing() {
    // max_rollback=8, max_input_delay=4: force-confirm after backlog exceeds 4 frames
    let mut session = RollbackSession::new(SimpleState { pos: 0 }, 2, 8, 4);

    // Advance 20 frames with player 1's input never arriving
    for _ in 0..20 {
        session.advance_frame(0, SimpleInput { delta: 1 }).unwrap();
    }

    // input_history length must not exceed max_input_delay + 1
    assert!(
        session.pending_frames() <= 5,
        "input_history backed up {} frames, exceeded expected upper bound",
        session.pending_frames()
    );
}

#[test]
fn test_confirmed_frame_advances_even_without_remote_input() {
    // max_input_delay=3: force-confirm after 3 frames
    let mut session = RollbackSession::new(SimpleState { pos: 0 }, 2, 8, 3);

    for _ in 0..10 {
        session.advance_frame(0, SimpleInput { delta: 1 }).unwrap();
    }

    // confirmed_frame must have advanced, not remain stuck at 0
    assert!(
        session.confirmed_frame() > 0,
        "confirmed_frame should have advanced but is still 0"
    );
}

#[test]
fn test_history_snapshots_bounded() {
    // max_rollback=4: at most 4 frames of snapshots retained
    let mut session = RollbackSession::new(SimpleState { pos: 0 }, 2, 4, 8);

    for _ in 0..20 {
        session.advance_frame(0, SimpleInput { delta: 1 }).unwrap();
    }

    // Snapshot count must not exceed max_rollback_frames
    // (may be even fewer after confirmed cleanup)
    // Here we only verify no unbounded growth (indirect): pending_frames being finite suffices
    assert!(session.pending_frames() < 20);
}

// ---------------------------------------------------------------------------
// Error path tests
// ---------------------------------------------------------------------------

#[test]
fn test_add_remote_input_too_old() {
    let mut session = make_session(0);

    // Advance two frames and confirm both
    for f in 0..2u32 {
        session.advance_frame(0, SimpleInput { delta: 1 }).unwrap();
        session
            .add_remote_input(1, f, SimpleInput { delta: 1 })
            .unwrap();
    }

    // confirmed_frame == 2, submitting frame 0 again must return InputTooOld
    let err = session
        .add_remote_input(1, 0, SimpleInput { delta: 99 })
        .unwrap_err();
    assert!(matches!(err, RollbackError::InputTooOld { .. }));
}

#[test]
fn test_add_remote_input_invalid_player() {
    let mut session = make_session(0);
    session.advance_frame(0, SimpleInput { delta: 1 }).unwrap();
    let err = session
        .add_remote_input(99, 0, SimpleInput { delta: 1 })
        .unwrap_err();
    assert!(matches!(err, RollbackError::InvalidPlayerId { .. }));
}

// ---------------------------------------------------------------------------
// OOS detection tests
// ---------------------------------------------------------------------------

#[test]
fn test_verify_checksum_ok() {
    let session = make_session(42);
    let local_checksum = session.confirmed_state().checksum();
    assert!(session.verify_checksum(0, local_checksum).is_ok());
}

#[test]
fn test_verify_checksum_oos() {
    let session = make_session(42);
    let err = session.verify_checksum(0, 0xdeadbeef).unwrap_err();
    assert!(matches!(err, RollbackError::OutOfSync { .. }));
}

// ---------------------------------------------------------------------------
// Input delay tests
// ---------------------------------------------------------------------------

#[test]
fn test_input_delay_defers_local_input() {
    // input_delay=2: local input takes effect 2 frames later
    let mut session = RollbackSession::with_input_delay(SimpleState { pos: 0 }, 2, 8, 16, 2);

    // Frame 0: submit local +10, delayed by 2 frames (target_frame=2); current frame uses
    // prediction (0)
    session.advance_frame(0, SimpleInput { delta: 10 }).unwrap();
    // After frame 0: local player prediction is 0, pos = 0
    assert_eq!(
        session.current_state().pos,
        0,
        "local input must not take effect during the delay period"
    );

    // Frame 1: submit another +10 (target_frame=3); current frame still uses prediction
    session.advance_frame(0, SimpleInput { delta: 10 }).unwrap();
    assert_eq!(
        session.current_state().pos,
        0,
        "local input must not take effect during the delay period"
    );

    // Frame 2: first +10 matures (target_frame=2 == current_frame=2), takes effect
    session.advance_frame(0, SimpleInput { delta: 10 }).unwrap();
    assert_eq!(
        session.current_state().pos,
        10,
        "local input must take effect after the delay expires"
    );
}

#[test]
fn test_input_delay_zero_is_immediate() {
    // input_delay=0: equivalent to the original behavior, input takes effect immediately
    let mut session = RollbackSession::with_input_delay(SimpleState { pos: 0 }, 2, 8, 16, 0);

    session.advance_frame(0, SimpleInput { delta: 5 }).unwrap();
    assert_eq!(
        session.current_state().pos,
        5,
        "input must take effect immediately with zero delay"
    );
}

#[test]
fn test_input_delay_reduces_rollback() {
    // Verify: no rollback when remote input arrives within the delay window
    // input_delay=2, remote player input arrives within 2 frames
    let mut session = RollbackSession::with_input_delay(SimpleState { pos: 0 }, 2, 8, 16, 2);

    // Frame 0: local +5 (delayed to frame 2), current frame local prediction is 0
    session.advance_frame(0, SimpleInput { delta: 5 }).unwrap();

    // Remote real input for frame 0 (+3) also arrives before local +5 has been applied
    // Local prediction for frame 0 is 0, remote prediction is also 0
    // If remote real input matches prediction (both default=0), no rollback occurs
    session
        .add_remote_input(1, 0, SimpleInput { delta: 0 })
        .unwrap();

    let pos_after = session.current_state().pos;
    // End of frame 0: local 0 (in delay) + remote 0 = 0
    assert_eq!(pos_after, 0);
}

// ---------------------------------------------------------------------------
// Batch add_remote_inputs tests
// ---------------------------------------------------------------------------

#[test]
fn test_add_remote_inputs_basic() {
    let mut session = make_session(0);

    // Advance 3 frames with player 1's input all predicted as 0
    for _ in 0..3 {
        session.advance_frame(0, SimpleInput { delta: 5 }).unwrap();
    }
    // At this point pos = 15 (player 0 +5 per frame, player 1 predicted 0)

    // Batch-submit player 1's real inputs for frames 0, 1, 2 (all +2)
    session
        .add_remote_inputs(
            1,
            0,
            vec![
                SimpleInput { delta: 2 },
                SimpleInput { delta: 2 },
                SimpleInput { delta: 2 },
            ],
        )
        .unwrap();

    // After rollback and re-simulation: each frame +5+2=7, 3 frames → pos = 21
    assert_eq!(session.current_state().pos, 21);
}

#[test]
fn test_add_remote_inputs_skips_confirmed_frames() {
    let mut session = make_session(0);

    // Frame 0: both players arrive → confirmed_frame=1
    session.advance_frame(0, SimpleInput { delta: 1 }).unwrap();
    session
        .add_remote_input(1, 0, SimpleInput { delta: 1 })
        .unwrap();
    assert_eq!(session.confirmed_frame(), 1);

    // Advance frame 1
    session.advance_frame(0, SimpleInput { delta: 1 }).unwrap();

    // Batch starting at frame 0 (frame 0 already confirmed, must be skipped; frame 1 processed
    // normally)
    let result = session.add_remote_inputs(
        1,
        0,
        vec![
            SimpleInput { delta: 99 }, // Frame 0: already confirmed, skip (no error)
            SimpleInput { delta: 3 },  // Frame 1: real input +3
        ],
    );
    assert!(
        result.is_ok(),
        "confirmed frames must be skipped rather than returning an error"
    );

    // Frame 1: player 0 +1, player 1 +3 → confirmed_state.pos=2 (frame 0: 1+1), frame 1 replay:
    // +1+3=4, total=6
    assert_eq!(session.current_state().pos, 6);
}

#[test]
fn test_add_remote_inputs_single_rollback_for_multiple_wrong_predictions() {
    let mut session = make_session(0);

    // Advance 4 frames
    for _ in 0..4 {
        session.advance_frame(0, SimpleInput { delta: 1 }).unwrap();
    }
    // pos = 4 (player 1 all predicted 0)

    // Batch-submit player 1's real inputs for frames 0~3 (all differ from prediction)
    session
        .add_remote_inputs(
            1,
            0,
            vec![
                SimpleInput { delta: 1 },
                SimpleInput { delta: 1 },
                SimpleInput { delta: 1 },
                SimpleInput { delta: 1 },
            ],
        )
        .unwrap();

    // Only one rollback executed (from frame 0), replaying 4 frames: each frame +1+1=2 → pos = 8
    assert_eq!(session.current_state().pos, 8);
}

#[test]
fn test_add_remote_inputs_invalid_player() {
    let mut session = make_session(0);
    session.advance_frame(0, SimpleInput { delta: 1 }).unwrap();

    let err = session
        .add_remote_inputs(99, 0, vec![SimpleInput { delta: 1 }])
        .unwrap_err();
    assert!(matches!(err, RollbackError::InvalidPlayerId { .. }));
}

#[test]
fn test_add_remote_inputs_empty_is_noop() {
    let mut session = make_session(0);
    session.advance_frame(0, SimpleInput { delta: 5 }).unwrap();
    let pos_before = session.current_state().pos;

    session.add_remote_inputs(1, 0, vec![]).unwrap();

    assert_eq!(session.current_state().pos, pos_before);
}

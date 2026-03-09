//! Property-based tests for the rollback engine using `proptest`.
//!
//! All tests use only the public API and are compiled as a separate
//! integration-test binary by Cargo.

use proptest::prelude::*;
use rollback_rs::error::RollbackError;
use rollback_rs::state::RollbackState;
use rollback_rs::sync::RollbackSession;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Shared test state (mirrors src/tests.rs)
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

// ---------------------------------------------------------------------------
// Strategy helpers
// ---------------------------------------------------------------------------

fn arb_delta() -> impl Strategy<Value = i32> {
    -100i32..=100i32
}

fn arb_inputs(n: usize) -> impl Strategy<Value = Vec<SimpleInput>> {
    proptest::collection::vec(arb_delta().prop_map(|d| SimpleInput { delta: d }), n)
}

// ---------------------------------------------------------------------------
// Properties
// ---------------------------------------------------------------------------

proptest! {
    /// Simulating N frames then providing real remote inputs must produce
    /// the same final state as simulating from scratch with those real inputs.
    #[test]
    fn prop_rollback_matches_fresh_sim(
        p0_deltas in arb_inputs(6),
        p1_deltas in arb_inputs(6),
    ) {
        // Reference: simulate with both inputs available from the start
        let mut reference = RollbackSession::new(SimpleState { pos: 0 }, 2, 8, 16);
        for (i, (d0, d1)) in p0_deltas.iter().zip(&p1_deltas).enumerate() {
            reference.advance_frame(0, d0.clone()).unwrap();
            reference.add_remote_input(1, i as u32, d1.clone()).unwrap();
        }

        // Rollback path: advance with predictions, then deliver real inputs
        let mut session = RollbackSession::new(SimpleState { pos: 0 }, 2, 8, 16);
        for d0 in &p0_deltas {
            session.advance_frame(0, d0.clone()).unwrap();
        }
        session.add_remote_inputs(1, 0, p1_deltas.clone()).unwrap();

        prop_assert_eq!(
            session.current_state().pos,
            reference.current_state().pos,
            "rollback path must produce the same state as fresh simulation"
        );
    }

    /// Batch delivery and one-by-one delivery must produce identical states.
    #[test]
    fn prop_batch_vs_individual_delivery(
        p0_deltas in arb_inputs(5),
        p1_deltas in arb_inputs(5),
    ) {
        // One-by-one delivery
        let mut s1 = RollbackSession::new(SimpleState { pos: 0 }, 2, 8, 16);
        for d in &p0_deltas {
            s1.advance_frame(0, d.clone()).unwrap();
        }
        for (i, d) in p1_deltas.iter().enumerate() {
            let _ = s1.add_remote_input(1, i as u32, d.clone());
        }

        // Batch delivery
        let mut s2 = RollbackSession::new(SimpleState { pos: 0 }, 2, 8, 16);
        for d in &p0_deltas {
            s2.advance_frame(0, d.clone()).unwrap();
        }
        s2.add_remote_inputs(1, 0, p1_deltas.clone()).unwrap();

        prop_assert_eq!(s1.current_state().pos, s2.current_state().pos);
    }

    /// confirmed_frame is monotonically non-decreasing across any input sequence.
    #[test]
    fn prop_confirmed_frame_monotonic(
        p0_deltas in arb_inputs(10),
        p1_deltas in arb_inputs(10),
    ) {
        let mut session = RollbackSession::new(SimpleState { pos: 0 }, 2, 8, 16);
        let mut last_confirmed = 0u32;

        for (i, (d0, d1)) in p0_deltas.iter().zip(&p1_deltas).enumerate() {
            session.advance_frame(0, d0.clone()).unwrap();
            let _ = session.add_remote_input(1, i as u32, d1.clone());

            let cf = session.confirmed_frame();
            prop_assert!(
                cf >= last_confirmed,
                "confirmed_frame went backwards: {} → {}",
                last_confirmed,
                cf
            );
            last_confirmed = cf;
        }
    }

    /// pending_frames stays within max_input_delay + 1 even when player 1
    /// never sends any inputs.
    #[test]
    fn prop_input_history_bounded(
        max_delay in 2u32..=8u32,
        frames   in 10u32..=40u32,
        p0_deltas in proptest::collection::vec(arb_delta(), 40),
    ) {
        let mut session = RollbackSession::new(SimpleState { pos: 0 }, 2, 8, max_delay);

        for delta in p0_deltas.iter().take(frames as usize) {
            session
                .advance_frame(0, SimpleInput { delta: *delta })
                .unwrap();
        }

        prop_assert!(
            session.pending_frames() <= (max_delay + 1) as usize,
            "pending_frames={} exceeded max_delay+1={}",
            session.pending_frames(),
            max_delay + 1
        );
    }

    /// Checksum is stable: two calls on the same state return the same value.
    #[test]
    fn prop_checksum_stable(pos in -100_000i32..=100_000i32) {
        let s = SimpleState { pos };
        prop_assert_eq!(s.checksum(), s.checksum());
    }

    /// Checksum is injective across the test domain.
    #[test]
    fn prop_checksum_injective(
        a in -1000i32..=1000i32,
        b in -1000i32..=1000i32,
    ) {
        prop_assume!(a != b);
        prop_assert_ne!(
            SimpleState { pos: a }.checksum(),
            SimpleState { pos: b }.checksum(),
            "checksum collision for pos={} and pos={}",
            a,
            b
        );
    }

    /// OOS is detected whenever the remote checksum differs from local.
    #[test]
    fn prop_oos_detected_on_mismatch(
        pos   in -100_000i32..=100_000i32,
        noise in 1u64..=u64::MAX,
    ) {
        let session = RollbackSession::new(SimpleState { pos }, 2, 8, 16);
        let local = session.confirmed_state().checksum();
        let corrupted = local.wrapping_add(noise); // noise ≥ 1, so always ≠ local
        prop_assert!(session.verify_checksum(0, corrupted).is_err());
    }

    /// InputTooOld is always returned for any frame below confirmed_frame.
    #[test]
    fn prop_input_too_old_below_confirmed(
        frames      in 1u32..=6u32,
        extra_delta in -50i32..=50i32,
    ) {
        let mut session = RollbackSession::new(SimpleState { pos: 0 }, 2, 8, 16);

        // Confirm `frames` frames by delivering both players' inputs
        for f in 0..frames {
            session.advance_frame(0, SimpleInput { delta: 1 }).unwrap();
            session
                .add_remote_input(1, f, SimpleInput { delta: 1 })
                .unwrap();
        }

        prop_assert_eq!(session.confirmed_frame(), frames);

        // Every frame below confirmed_frame must be rejected
        for stale in 0..frames {
            let result =
                session.add_remote_input(1, stale, SimpleInput { delta: extra_delta });
            prop_assert!(
                matches!(result, Err(RollbackError::InputTooOld { .. })),
                "expected InputTooOld for stale frame {stale}, got {result:?}"
            );
        }
    }

    /// input_delay = 0 must be identical to the no-delay constructor.
    #[test]
    fn prop_zero_delay_equivalent_to_no_delay(deltas in arb_inputs(8)) {
        let mut s_no_delay =
            RollbackSession::new(SimpleState { pos: 0 }, 2, 8, 16);
        let mut s_zero =
            RollbackSession::with_input_delay(SimpleState { pos: 0 }, 2, 8, 16, 0);

        for d in &deltas {
            s_no_delay.advance_frame(0, d.clone()).unwrap();
            s_zero.advance_frame(0, d.clone()).unwrap();
        }

        prop_assert_eq!(s_no_delay.current_state().pos, s_zero.current_state().pos);
        prop_assert_eq!(s_no_delay.current_frame(), s_zero.current_frame());
    }
}

use std::collections::VecDeque;

use crate::error::RollbackError;
use crate::state::RollbackState;

/// Stores all player inputs for a specific frame.
#[derive(Clone)]
struct FrameInputs<I> {
    frame: u32,
    inputs: Vec<I>,
    /// Records which inputs are predicted and which are real/confirmed.
    is_predicted: Vec<bool>,
}

impl<I> FrameInputs<I> {
    /// Returns true if all inputs for this frame are confirmed.
    fn is_fully_confirmed(&self) -> bool {
        self.is_predicted.iter().all(|&p| !p)
    }
}

/// Rollback session main structure, containing state snapshots and input histories.
pub struct RollbackSession<S: RollbackState> {
    /// Emulator's current state (may include predicted inputs).
    current_state: S,
    /// Current simulated frame number.
    current_frame: u32,

    /// Latest confirmed state (all player inputs received).
    confirmed_state: S,
    /// Frame number for confirmed state.
    confirmed_frame: u32,

    /// Snapshot buffer: (frame number, state snapshot)
    history: VecDeque<(u32, S)>,

    /// Input history: all player inputs for each frame.
    input_history: VecDeque<FrameInputs<S::Input>>,

    /// Number of players.
    player_count: usize,

    /// Maximum number of frames allowed for rollback.
    max_rollback_frames: u32,

    /// A frame accumulated in this queue longer than this number will be forcibly marked as
    /// confirmed. Prevents infinite growth of input_history. Recommended: 2~3 times
    /// max_rollback_frames.
    max_input_delay: u32,

    /// Input delay frames: local input is delayed N frames before simulation.
    /// 0 disables delay (normal behavior).
    /// 2~4 is typical for network games.
    input_delay: u32,

    /// Local input buffer queue: (target frame, player ID, input)
    /// Local inputs are placed here and only sent into simulation when current_frame >= target
    /// frame.
    local_input_queue: VecDeque<(u32, usize, S::Input)>,
}

impl<S: RollbackState> RollbackSession<S> {
    /// Creates a new session.
    pub fn new(
        initial_state: S,
        player_count: usize,
        max_rollback: u32,
        max_input_delay: u32,
    ) -> Self {
        Self::with_input_delay(
            initial_state,
            player_count,
            max_rollback,
            max_input_delay,
            0,
        )
    }

    /// Creates a session with input delay.
    ///
    /// - `input_delay`: local input is delayed this many frames before simulation. Recommended:
    ///   close to RTT/2 frame count, usually 2~4 frames.
    pub fn with_input_delay(
        initial_state: S,
        player_count: usize,
        max_rollback: u32,
        max_input_delay: u32,
        input_delay: u32,
    ) -> Self {
        Self {
            current_state: initial_state.clone(),
            current_frame: 0,
            confirmed_state: initial_state,
            confirmed_frame: 0,
            history: VecDeque::new(),
            input_history: VecDeque::new(),
            player_count,
            max_rollback_frames: max_rollback,
            max_input_delay,
            input_delay,
            local_input_queue: VecDeque::new(),
        }
    }

    // -------------------------------------------------------------------------
    // Getter
    // -------------------------------------------------------------------------

    /// Returns a reference to the current (possibly predicted) state.
    pub fn current_state(&self) -> &S {
        &self.current_state
    }

    /// Returns a reference to the latest fully confirmed state.
    pub fn confirmed_state(&self) -> &S {
        &self.confirmed_state
    }

    /// Returns the current simulated frame number.
    pub fn current_frame(&self) -> u32 {
        self.current_frame
    }

    /// Returns the frame number of the last confirmed state.
    pub fn confirmed_frame(&self) -> u32 {
        self.confirmed_frame
    }

    /// Returns the number of players.
    pub fn player_count(&self) -> usize {
        self.player_count
    }

    /// Returns the configured input delay in frames.
    pub fn input_delay(&self) -> u32 {
        self.input_delay
    }

    /// Returns the number of frames currently pending in input_history.
    pub fn pending_frames(&self) -> usize {
        self.input_history.len()
    }

    // -------------------------------------------------------------------------
    // Core APIs
    // -------------------------------------------------------------------------

    /// Advances simulation by one frame: applies local input (with delay), and predicts remote
    /// input using "repeat last frame" strategy.
    ///
    /// If `input_delay > 0`, `local_input` will be temporarily stored.
    /// The simulation includes the local input only after the designated delay.
    /// Until then, both local and remote player inputs are filled using "repeat last frame"
    /// prediction.
    pub fn advance_frame(
        &mut self,
        local_player_id: usize,
        local_input: S::Input,
    ) -> Result<(), RollbackError> {
        if local_player_id >= self.player_count {
            return Err(RollbackError::InvalidPlayerId {
                player_id: local_player_id,
                player_count: self.player_count,
            });
        }

        // Put local input into the delay queue, target frame = current frame + input_delay
        let target_frame = self.current_frame + self.input_delay;
        self.local_input_queue
            .push_back((target_frame, local_player_id, local_input));

        self.advance_frame_inner(local_player_id)
    }

    /// Receives a single confirmed remote input for a specific frame.
    pub fn add_remote_input(
        &mut self,
        player_id: usize,
        frame: u32,
        input: S::Input,
    ) -> Result<(), RollbackError> {
        if player_id >= self.player_count {
            return Err(RollbackError::InvalidPlayerId {
                player_id,
                player_count: self.player_count,
            });
        }

        if frame < self.confirmed_frame {
            return Err(RollbackError::InputTooOld {
                frame,
                confirmed_frame: self.confirmed_frame,
            });
        }

        self.apply_remote_input(player_id, frame, input)?;
        self.update_confirmed_state();

        Ok(())
    }

    /// Receives multiple remote inputs starting from `start_frame` (batch submission).
    ///
    /// Network packets often carry redundant multi-frame input to deal with packet loss; this API
    /// processes those in batch. Finds the earliest frame that needs a rollback and executes it
    /// just once to avoid repeatedly rolling back.
    pub fn add_remote_inputs(
        &mut self,
        player_id: usize,
        start_frame: u32,
        inputs: Vec<S::Input>,
    ) -> Result<(), RollbackError> {
        if player_id >= self.player_count {
            return Err(RollbackError::InvalidPlayerId {
                player_id,
                player_count: self.player_count,
            });
        }

        if inputs.is_empty() {
            return Ok(());
        }

        // Find the earliest rollback frame (prediction error, minimum frame index).
        let mut earliest_rollback_frame: Option<u32> = None;

        for (offset, input) in inputs.iter().enumerate() {
            let frame = start_frame + offset as u32;

            if frame < self.confirmed_frame {
                // Already confirmed frames are skipped (not an error; batches are often redundant).
                continue;
            }

            if let Some(history_idx) = self.input_history.iter().position(|h| h.frame == frame) {
                let h = &self.input_history[history_idx];
                let need_rollback = h.is_predicted[player_id] && h.inputs[player_id] != *input;

                if need_rollback {
                    earliest_rollback_frame = Some(match earliest_rollback_frame {
                        None => frame,
                        Some(prev) => prev.min(frame),
                    });
                }
            }

            // Update input record (regardless if rollback is needed).
            if let Some(history_idx) = self.input_history.iter().position(|h| h.frame == frame) {
                let h = &mut self.input_history[history_idx];
                h.inputs[player_id] = input.clone();
                h.is_predicted[player_id] = false;
            }
        }

        // Only perform rollback once (from the earliest error frame).
        if let Some(rollback_frame) = earliest_rollback_frame {
            self.rollback_to(rollback_frame)?;
        }

        self.update_confirmed_state();

        Ok(())
    }

    /// Out-of-sync check: compares local checksum for a frame with a remote provided checksum.
    pub fn verify_checksum(&self, frame: u32, remote_checksum: u64) -> Result<(), RollbackError> {
        if frame == self.confirmed_frame {
            let local = self.confirmed_state.checksum();
            if local != remote_checksum {
                return Err(RollbackError::OutOfSync {
                    frame,
                    local,
                    remote: remote_checksum,
                });
            }
        }
        Ok(())
    }

    // -------------------------------------------------------------------------
    // Internal Methods
    // -------------------------------------------------------------------------

    /// Internal logic for advancing one frame.
    fn advance_frame_inner(&mut self, local_player_id: usize) -> Result<(), RollbackError> {
        let frame = self.current_frame;

        // 1. Prepare inputs for current frame: use delayed local input if available, else predict.
        let mut frame_inputs = vec![S::Input::default(); self.player_count];
        let mut is_predicted = vec![true; self.player_count];

        // Use available input from delay queue (target_frame == current_frame)
        if let Some(pos) = self
            .local_input_queue
            .iter()
            .position(|(target, pid, _)| *target == frame && *pid == local_player_id)
        {
            let (_, _, input) = self.local_input_queue.remove(pos).unwrap();
            frame_inputs[local_player_id] = input;
            is_predicted[local_player_id] = false;
        }
        // If not available yet (still delaying), fill using last frame prediction

        // Remote players (and delayed local) are filled with last frame prediction
        if let Some(prev) = self.input_history.back() {
            for p in 0..self.player_count {
                if is_predicted[p] {
                    frame_inputs[p] = prev.inputs[p].clone();
                }
            }
        }

        // 2. Save a snapshot of current state (before advancing)
        if self.history.len() >= self.max_rollback_frames as usize {
            self.history.pop_front();
        }
        self.history.push_back((frame, self.current_state.clone()));

        // 3. Advance state and record input history
        self.current_state.advance(&frame_inputs);
        self.input_history.push_back(FrameInputs {
            frame,
            inputs: frame_inputs,
            is_predicted,
        });
        self.current_frame += 1;

        // 4. After each advance, try to clear confirmed frames.
        self.update_confirmed_state();

        Ok(())
    }

    /// Applies a remote input (with rollback check); does not call update_confirmed_state.
    fn apply_remote_input(
        &mut self,
        player_id: usize,
        frame: u32,
        input: S::Input,
    ) -> Result<(), RollbackError> {
        if let Some(history_idx) = self.input_history.iter().position(|h| h.frame == frame) {
            let need_rollback = {
                let h = &self.input_history[history_idx];
                h.is_predicted[player_id] && h.inputs[player_id] != input
            };

            {
                let h = &mut self.input_history[history_idx];
                h.inputs[player_id] = input;
                h.is_predicted[player_id] = false;
            }

            if need_rollback {
                self.rollback_to(frame)?;
            }
        }
        Ok(())
    }

    /// Rollback to snapshot at target_frame, then replay forward to current frame.
    /// While replaying, update snapshots for subsequent frames to ensure correctness for future
    /// rollbacks.
    fn rollback_to(&mut self, target_frame: u32) -> Result<(), RollbackError> {
        let rollback_frames = self.current_frame.saturating_sub(target_frame);
        if rollback_frames > self.max_rollback_frames {
            return Err(RollbackError::RollbackTooFar {
                needed: rollback_frames,
                max: self.max_rollback_frames,
            });
        }

        let snapshot = self
            .history
            .iter()
            .find(|(f, _)| *f == target_frame)
            .map(|(_, s)| s.clone())
            .ok_or(RollbackError::SnapshotNotFound {
                frame: target_frame,
            })?;

        self.current_state = snapshot;

        let start_idx = self
            .input_history
            .iter()
            .position(|h| h.frame == target_frame)
            .ok_or(RollbackError::SnapshotNotFound {
                frame: target_frame,
            })?;

        for i in start_idx..self.input_history.len() {
            let frame = self.input_history[i].frame;

            // Update snapshot before replay to ensure future rollbacks to this frame behave
            // correctly
            if let Some(entry) = self.history.iter_mut().find(|(f, _)| *f == frame) {
                entry.1 = self.current_state.clone();
            }

            self.current_state.advance(&self.input_history[i].inputs);
        }

        Ok(())
    }

    /// Advances confirmed_state:
    /// - Normal: oldest frame for which all player inputs are confirmed, advance step by step.
    /// - Timeout: a frame left in the queue longer than max_input_delay is forcibly confirmed.
    fn update_confirmed_state(&mut self) {
        loop {
            let should_confirm = match self.input_history.front() {
                None => false,
                Some(h) => {
                    if h.is_fully_confirmed() {
                        true
                    } else {
                        let age = self.current_frame.saturating_sub(h.frame);
                        age > self.max_input_delay
                    }
                }
            };

            if !should_confirm {
                break;
            }

            let frame_record = self.input_history.pop_front().unwrap();
            self.confirmed_state.advance(&frame_record.inputs);
            self.confirmed_frame = frame_record.frame + 1;

            while self
                .history
                .front()
                .map(|(f, _)| *f < self.confirmed_frame)
                .unwrap_or(false)
            {
                self.history.pop_front();
            }
        }
    }
}

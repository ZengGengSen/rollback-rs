//! State management for a single remote peer

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

pub struct PeerState {
    pub addr: SocketAddr,
    pub player_id: usize,
    pub rtt_ms: f32,
    last_ping_at: Option<Instant>,
    pub remote_confirmed_frame: u32,
    pub redundancy: usize,
    sent_input_history: VecDeque<(u32, Vec<u8>)>,
    max_sent_history: usize,
}

impl PeerState {
    pub fn new(addr: SocketAddr, player_id: usize) -> Self {
        Self {
            addr,
            player_id,
            rtt_ms: 100.0,
            last_ping_at: None,
            remote_confirmed_frame: 0,
            redundancy: 3,
            sent_input_history: VecDeque::new(),
            max_sent_history: 32,
        }
    }

    pub fn on_ping_sent(&mut self) {
        self.last_ping_at = Some(Instant::now());
    }

    pub fn on_pong_received(&mut self) {
        if let Some(sent_at) = self.last_ping_at.take() {
            let rtt = sent_at.elapsed().as_millis() as f32;
            self.rtt_ms = 0.875 * self.rtt_ms + 0.125 * rtt;
            self.update_redundancy();
        }
    }

    fn update_redundancy(&mut self) {
        let frame_duration_ms = 1000.0 / 60.0;
        let rtt_frames = (self.rtt_ms / frame_duration_ms).ceil() as usize;
        self.redundancy = (rtt_frames + 2).max(2).min(16);
    }

    pub fn suggested_input_delay(&self) -> u32 {
        let frame_duration_ms = 1000.0 / 60.0;
        let half_rtt_frames = (self.rtt_ms / 2.0 / frame_duration_ms).ceil() as u32;
        half_rtt_frames.min(8)
    }

    pub fn record_sent_input(&mut self, frame: u32, input_bytes: Vec<u8>) {
        self.sent_input_history.push_back((frame, input_bytes));
        while self.sent_input_history.len() > self.max_sent_history {
            self.sent_input_history.pop_front();
        }
    }

    /// Returns the redundant inputs to include in the current send.
    /// Returns (start_frame, [input_bytes...]), ordered from the oldest redundant frame.
    pub fn get_redundant_inputs(&self, current_frame: u32) -> (u32, Vec<Vec<u8>>) {
        if self.sent_input_history.is_empty() {
            return (current_frame, vec![]);
        }

        // Take the most recent `redundancy` entries, capped at the actual history length
        let count = self.redundancy.min(self.sent_input_history.len());
        let start_idx = self.sent_input_history.len() - count; // no underflow: count <= len

        let frames: Vec<(u32, Vec<u8>)> = self
            .sent_input_history
            .iter()
            .skip(start_idx)
            .cloned()
            .collect();

        let start_frame = frames.first().map(|(f, _)| *f).unwrap_or(current_frame);
        (start_frame, frames.into_iter().map(|(_, b)| b).collect())
    }

    pub fn frame_advantage(&self, local_frame: u32) -> i32 {
        local_frame as i32 - self.remote_confirmed_frame as i32
    }

    pub fn should_stall(&self, local_frame: u32, max_rollback_frames: u32) -> bool {
        self.frame_advantage(local_frame) > max_rollback_frames as i32
    }

    pub fn update_remote_confirmed(&mut self, frame: u32) {
        if frame > self.remote_confirmed_frame {
            self.remote_confirmed_frame = frame;
        }
    }

    pub fn rtt(&self) -> Duration {
        Duration::from_millis(self.rtt_ms as u64)
    }
}

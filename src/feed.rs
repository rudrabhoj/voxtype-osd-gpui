//! Audio level history from the daemon's 100 Hz frames, played out on a fixed
//! delay so bars scroll smoothly even though frames arrive in 10-40 ms batches.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub const FRAME_HZ: f64 = 100.0;
const DELAY_FRAMES: f64 = 6.0;
const KEEP_FRAMES: usize = 400;

/// dBFS that maps to an empty bar, and the span up to a full one. A laptop mic
/// idles around -55 dBFS and speech peaks at -25..-6.
const FLOOR_DB: f32 = -52.0;
const SPAN_DB: f32 = 46.0;

pub fn normalize(peak_dbfs: f32) -> f32 {
    ((peak_dbfs - FLOOR_DB) / SPAN_DB).clamp(0.0, 1.0).powf(1.4)
}

#[derive(Default)]
pub struct Feed {
    levels: VecDeque<f32>,
    /// Session index of `levels[0]`.
    base: u64,
    count: u64,
    last_seq: Option<u32>,
    origin: Option<Instant>,
}

impl Feed {
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn push(&mut self, seq: u32, peak_dbfs: f32, now: Instant) {
        // The daemon restarts seq at 0 for every recording.
        if self.last_seq.is_some_and(|last| seq != last.wrapping_add(1)) {
            self.clear();
        }
        self.last_seq = Some(seq);
        let origin = *self.origin.get_or_insert(now);
        let elapsed = now.duration_since(origin).as_secs_f64() * FRAME_HZ;
        if elapsed > self.count as f64 + DELAY_FRAMES {
            // Capture stalled: resume from where the display stopped instead of
            // jumping through the late batch.
            let resume = self.count.saturating_sub(1) as f64 + DELAY_FRAMES;
            self.origin = Some(now - Duration::from_secs_f64(resume / FRAME_HZ));
        }
        self.levels.push_back(normalize(peak_dbfs));
        self.count += 1;
        if self.levels.len() > KEEP_FRAMES {
            self.levels.pop_front();
            self.base += 1;
        }
    }

    /// Display position in frames, never ahead of the newest frame.
    fn head(&self, now: Instant) -> Option<f64> {
        let origin = self.origin?;
        let t = now.saturating_duration_since(origin).as_secs_f64() * FRAME_HZ - DELAY_FRAMES;
        Some(t.clamp(0.0, (self.count - 1) as f64))
    }

    fn level_at(&self, index: i64) -> f32 {
        if index < self.base as i64 {
            return 0.0;
        }
        self.levels.get((index - self.base as i64) as usize).copied().unwrap_or(0.0)
    }

    /// `n` bars, oldest first, each the loudest of `per_bar` frames, plus how
    /// far (0..1 of a bar pitch) the row has scrolled left since the newest bar
    /// started.
    pub fn bars(&self, now: Instant, n: usize, per_bar: usize) -> (Vec<f32>, f32) {
        let Some(head) = self.head(now) else {
            return (vec![0.0; n], 0.0);
        };
        let per = per_bar as f64;
        let newest = (head / per).floor() as i64;
        let scroll = (head / per - newest as f64) as f32;
        let last = head.floor() as i64;
        let bars = (0..n)
            .map(|i| {
                let bucket = newest - (n - 1 - i) as i64;
                let start = bucket * per_bar as i64;
                (start..(start + per_bar as i64).min(last + 1))
                    .map(|k| self.level_at(k))
                    .fold(0.0, f32::max)
            })
            .collect();
        (bars, scroll)
    }

    /// Loudness right now (last 40 ms at the playout head), 0..1.
    pub fn level(&self, now: Instant) -> f32 {
        let Some(head) = self.head(now) else { return 0.0 };
        let last = head.floor() as i64;
        (last - 3..=last).map(|k| self.level_at(k)).fold(0.0, f32::max)
    }
}

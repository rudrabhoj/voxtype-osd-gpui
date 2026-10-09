//! Turns daemon state + audio history + time into one frame's worth of visual
//! state. Pure (time is passed in), so the offscreen preview renders exactly
//! what the live overlay does.

use std::time::{Duration, Instant};

use crate::feed::Feed;

pub const BARS: usize = 36;
pub const FRAMES_PER_BAR: usize = 4;

const APPEAR_SECS: f32 = 0.22;
const CLOSE_SECS: f32 = 0.24;
const MODE_SECS: f32 = 0.35;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Daemon {
    Idle,
    Recording,
    Transcribing,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    /// Whole-overlay opacity (entrance and exit fades).
    pub alpha: f32,
    /// 0..1 entrance progress, eased; bars and glow grow with it.
    pub appear: f32,
    /// 0 = recording look, 1 = transcribing look.
    pub transcribing: f32,
    /// Smoothed loudness 0..1.
    pub level: f32,
    /// Bar heights 0..1, oldest first.
    pub bars: Vec<f32>,
    /// Fraction of a bar pitch the row has scrolled left.
    pub scroll: f32,
    pub elapsed: Duration,
    /// Seconds since the overlay appeared; drives idle animation.
    pub t: f32,
}

#[derive(Default)]
pub struct Presenter {
    shown_at: Option<Instant>,
    closing_since: Option<Instant>,
    closing_from: f32,
    rec_started: Option<Instant>,
    rec_stopped: Option<Instant>,
    last_state: Option<Daemon>,
    last_frame: Option<Instant>,
    level: f32,
    mode: f32,
    frozen_bars: Option<(Vec<f32>, f32)>,
}

fn ease_out_cubic(x: f32) -> f32 {
    1.0 - (1.0 - x.clamp(0.0, 1.0)).powi(3)
}

fn secs(a: Instant, b: Instant) -> f32 {
    b.saturating_duration_since(a).as_secs_f32()
}

impl Presenter {
    pub fn is_shown(&self) -> bool {
        self.shown_at.is_some()
    }

    /// `None` means the overlay is fully hidden and its window can go.
    pub fn frame(&mut self, now: Instant, state: Daemon, feed: &mut Feed) -> Option<Snapshot> {
        let previous = self.last_state.replace(state);
        let dt = self.last_frame.map_or(0.0, |t| secs(t, now)).min(0.1);
        self.last_frame = Some(now);

        if state == Daemon::Recording && previous != Some(Daemon::Recording) {
            // Old history must not scroll by at the start of a new recording.
            feed.clear();
            self.rec_started = Some(now);
            self.rec_stopped = None;
            self.frozen_bars = None;
            self.closing_since = None;
            if self.shown_at.is_none() {
                self.shown_at = Some(now);
                self.mode = 0.0;
                self.level = 0.0;
            }
        }
        if state != Daemon::Recording && previous == Some(Daemon::Recording) {
            self.rec_stopped = Some(now);
        }

        let shown_at = self.shown_at?;
        if state == Daemon::Idle && self.closing_since.is_none() {
            self.closing_since = Some(now);
            self.closing_from = ease_out_cubic(secs(shown_at, now) / APPEAR_SECS);
        }

        let appear = ease_out_cubic(secs(shown_at, now) / APPEAR_SECS);
        let alpha = match self.closing_since {
            Some(since) => {
                let k = secs(since, now) / CLOSE_SECS;
                if k >= 1.0 {
                    *self = Self::default();
                    return None;
                }
                self.closing_from * (1.0 - ease_out_cubic(k))
            }
            None => appear,
        };

        let target_mode = if state == Daemon::Transcribing { 1.0 } else { 0.0 };
        let step = dt / MODE_SECS;
        self.mode = if target_mode > self.mode {
            (self.mode + step).min(1.0)
        } else {
            (self.mode - step).max(0.0)
        };

        // Recording: live bars. After the mic stops, keep the last row still
        // so the transcribing wave grows out of what was just said.
        let (bars, scroll) = if state == Daemon::Recording {
            feed.bars(now, BARS, FRAMES_PER_BAR)
        } else {
            self.frozen_bars
                .get_or_insert_with(|| feed.bars(now, BARS, FRAMES_PER_BAR))
                .clone()
        };

        let target_level = if state == Daemon::Recording { feed.level(now) } else { 0.0 };
        let rate = if target_level > self.level { 28.0 } else { 7.0 };
        self.level += (target_level - self.level) * (1.0 - (-dt * rate).exp());

        let rec_started = self.rec_started.unwrap_or(now);
        let elapsed = self.rec_stopped.unwrap_or(now).saturating_duration_since(rec_started);

        Some(Snapshot {
            alpha,
            appear,
            transcribing: self.mode * self.mode * (3.0 - 2.0 * self.mode),
            level: self.level,
            bars,
            scroll,
            elapsed,
            t: secs(shown_at, now),
        })
    }
}

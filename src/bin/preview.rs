//! Render the overlay offscreen to a PNG sequence, driven by a real speech WAV
//! (16 kHz mono s16, the daemon's own capture format) through the same
//! presenter and view as the live overlay.
//!
//! voxtype-osd-preview <speech.wav> <out-dir> [recording-seconds]

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    AppContext, Context, Entity, HeadlessAppContext, IntoElement, ParentElement, Render, Styled,
    Window, black, div, px, size, white,
};

use voxtype_osd_gpui::feed::Feed;
use voxtype_osd_gpui::presenter::{Daemon, Presenter};
use voxtype_osd_gpui::view::{HEIGHT, Osd, WIDTH};

const FPS: f64 = 60.0;

/// The headless renderer has no alpha channel (it draws onto opaque black), so
/// each frame is drawn over black and over white and the alpha recovered from
/// the difference.
struct Stage {
    osd: Entity<Osd>,
    on_white: Rc<Cell<bool>>,
}

impl Render for Stage {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let bg = if self.on_white.get() { white() } else { black() };
        div().size_full().bg(bg).child(self.osd.clone())
    }
}

fn unmix(on_black: &image::RgbaImage, on_white: &image::RgbaImage) -> image::RgbaImage {
    let mut out = on_black.clone();
    for (o, (b, w)) in out.pixels_mut().zip(on_black.pixels().zip(on_white.pixels())) {
        let a = (0..3).map(|c| 255.0 - (w[c] as f32 - b[c] as f32)).sum::<f32>() / 3.0;
        let a = a.clamp(0.0, 255.0);
        for c in 0..3 {
            o[c] = if a > 0.5 { (b[c] as f32 * 255.0 / a).min(255.0) as u8 } else { 0 };
        }
        o[3] = a as u8;
    }
    out
}
const TRANSCRIBE_SECS: f64 = 1.6;

/// Peak dBFS per 10 ms (160 samples), as the daemon's LevelBucketer computes it.
fn wav_frames(path: &str) -> Vec<f32> {
    let bytes = std::fs::read(path).expect("read wav");
    let data = bytes.windows(4).position(|w| w == b"data").expect("no data chunk") + 8;
    bytes[data..]
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
        .collect::<Vec<_>>()
        .chunks_exact(160)
        .map(|c| {
            let peak = c.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            if peak <= 1e-6 { -120.0 } else { 20.0 * peak.log10() }
        })
        .collect()
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let (wav, out) = (&args[1], &args[2]);
    let rec_secs: f64 = args.get(3).map_or(6.0, |s| s.parse().unwrap());
    std::fs::create_dir_all(out)?;
    let peaks = wav_frames(wav);

    let text = Arc::new(gpui_wgpu::CosmicTextSystem::new("Maple Mono NF"));
    let mut cx = HeadlessAppContext::with_platform(text, Arc::new(()), || {
        gpui_platform::current_headless_renderer()
    });
    let t0 = Instant::now();
    let at = |secs: f64| t0 + Duration::from_secs_f64(secs);
    let mut feed = Feed::default();
    let mut presenter = Presenter::default();

    // The view closes its window when it has nothing to show, so it needs a
    // snapshot before the window opens.
    let current = Rc::new(RefCell::new(presenter.frame(at(0.0), Daemon::Recording, &mut feed)));
    let shown = current.clone();
    let on_white = Rc::new(Cell::new(false));
    let stage_white = on_white.clone();
    let window = cx.open_window(size(px(WIDTH), px(HEIGHT)), move |_, cx| {
        let osd = cx.new(|_| Osd::new(move || shown.borrow().clone()));
        cx.new(|_| Stage { osd, on_white: stage_white })
    })?;
    let mut delivered = 0usize;
    let mut next_batch = 0.0f64;
    let mut batch_no = 0u32;

    for n in 0.. {
        let secs = n as f64 / FPS;
        let state = if secs < rec_secs {
            Daemon::Recording
        } else if secs < rec_secs + TRANSCRIBE_SECS {
            Daemon::Transcribing
        } else {
            Daemon::Idle
        };
        // The first presenter frame clears the feed, as a new recording does live.
        let snapshot = presenter.frame(at(secs), state, &mut feed);
        // Capture hands frames over in 20-40 ms batches, a little late.
        while state == Daemon::Recording && next_batch <= secs {
            let frames_due = ((next_batch * 100.0) as usize).min(peaks.len());
            for (seq, &peak) in peaks.iter().enumerate().take(frames_due).skip(delivered) {
                feed.push(seq as u32, peak, at(next_batch + 0.004));
            }
            delivered = frames_due;
            batch_no += 1;
            next_batch += [0.02, 0.03, 0.04, 0.02][batch_no as usize % 4];
        }
        let Some(snapshot) = snapshot else { break };
        *current.borrow_mut() = Some(snapshot);

        let mut shots = Vec::with_capacity(2);
        for white_bg in [false, true] {
            on_white.set(white_bg);
            cx.update_window(window.into(), |_, window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            })?;
            shots.push(cx.capture_screenshot(window.into())?);
        }
        unmix(&shots[0], &shots[1]).save(format!("{out}/f{n:05}.png"))?;
    }
    Ok(())
}

//! voxtype on-screen display, built on GPUI.
//!
//! Reads the daemon's level socket ($XDG_RUNTIME_DIR/voxtype/audio.sock, 16-byte
//! frames at 100 Hz while recording) and its state file, and shows a layer-shell
//! overlay from the start of a recording until transcription finishes.

use std::cell::Cell;
use std::io::Read;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::layer_shell::{Anchor, KeyboardInteractivity, Layer, LayerShellOptions};
use gpui::{
    App, AppContext, Bounds, QuitMode, WindowBackgroundAppearance, WindowBounds, WindowKind,
    WindowOptions, point, px, size,
};

use voxtype_osd_gpui::feed::Feed;
use voxtype_osd_gpui::presenter::{Daemon, Presenter};
use voxtype_osd_gpui::view::{HEIGHT, Osd, WIDTH};

const BOTTOM_MARGIN: f32 = 44.0;
const POLL: Duration = Duration::from_millis(30);

fn runtime_dir() -> PathBuf {
    let base = std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR is not set");
    PathBuf::from(base).join("voxtype")
}

fn daemon_state(dir: &PathBuf) -> Daemon {
    // `record start --no-osd` leaves this marker for the whole recording.
    if dir.join("osd_suppressed").exists() {
        return Daemon::Idle;
    }
    match std::fs::read_to_string(dir.join("state")).as_deref().map(str::trim) {
        Ok("recording" | "streaming") => Daemon::Recording,
        Ok("transcribing") => Daemon::Transcribing,
        _ => Daemon::Idle,
    }
}

fn read_frames(socket: PathBuf, feed: Arc<Mutex<Feed>>) {
    loop {
        if let Ok(mut stream) = UnixStream::connect(&socket) {
            let mut buf = [0u8; 16];
            while stream.read_exact(&mut buf).is_ok() {
                let seq = u32::from_ne_bytes(buf[0..4].try_into().unwrap());
                let peak = f32::from_ne_bytes(buf[12..16].try_into().unwrap());
                feed.lock().unwrap().push(seq, peak, Instant::now());
            }
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn open_overlay(cx: &mut App, feed: Arc<Mutex<Feed>>, state: Rc<Cell<Daemon>>, open: Rc<Cell<bool>>) {
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
            point(px(0.), px(0.)),
            size(px(WIDTH), px(HEIGHT)),
        ))),
        titlebar: None,
        focus: false,
        kind: WindowKind::LayerShell(LayerShellOptions {
            namespace: "voxtype-osd".into(),
            layer: Layer::Overlay,
            anchor: Anchor::BOTTOM,
            margin: Some((px(0.), px(0.), px(BOTTOM_MARGIN), px(0.))),
            keyboard_interactivity: KeyboardInteractivity::None,
            ..Default::default()
        }),
        window_background: WindowBackgroundAppearance::Transparent,
        app_id: Some("voxtype-osd".into()),
        // The overlay never has focus; unthrottled so it animates at refresh rate.
        inactive_frame_interval: None,
        is_resizable: false,
        is_movable: false,
        ..Default::default()
    };
    let mut presenter = Presenter::default();
    let closed = open.clone();
    let result = cx.open_window(options, move |window, cx| {
        window.set_input_region(Some(&[]));
        cx.new(|_| {
            Osd::new(move || {
                let snapshot =
                    presenter.frame(Instant::now(), state.get(), &mut feed.lock().unwrap());
                if snapshot.is_none() {
                    closed.set(false);
                }
                snapshot
            })
        })
    });
    if let Err(e) = result {
        eprintln!("voxtype-osd-gpui: cannot open overlay: {e:#}");
        open.set(false);
    }
}

fn main() {
    let dir = runtime_dir();
    let feed = Arc::new(Mutex::new(Feed::default()));
    {
        let feed = feed.clone();
        let socket = dir.join("audio.sock");
        std::thread::Builder::new()
            .name("audio-socket".into())
            .spawn(move || read_frames(socket, feed))
            .expect("spawn socket thread");
    }

    gpui_platform::application()
        .with_quit_mode(QuitMode::Explicit)
        .run(move |cx: &mut App| {
            let state = Rc::new(Cell::new(Daemon::Idle));
            let open = Rc::new(Cell::new(false));
            cx.spawn(async move |cx| {
                loop {
                    let now = daemon_state(&dir);
                    state.set(now);
                    if now == Daemon::Recording && !open.get() {
                        open.set(true);
                        let (feed, state, open) = (feed.clone(), state.clone(), open.clone());
                        cx.update(|cx| open_overlay(cx, feed, state, open));
                    }
                    cx.background_executor().timer(POLL).await;
                }
            })
            .detach();
        });
}

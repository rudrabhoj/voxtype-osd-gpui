//! voxtype on-screen display, built on GPUI.
//!
//! Reads the daemon's level socket (<runtime dir>/audio.sock, 16-byte frames at
//! 100 Hz while recording) and its state file, and shows an overlay from the start
//! of a recording until transcription finishes: a wlr-layer-shell surface on Linux,
//! a non-activating floating panel on macOS.

use std::cell::Cell;
use std::io::Read;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::{
    App, AppContext, Bounds, QuitMode, Window, WindowBackgroundAppearance, WindowBounds,
    WindowKind, WindowOptions, point, px, size,
};

use voxtype_osd_gpui::feed::Feed;
use voxtype_osd_gpui::presenter::{Daemon, Presenter};
use voxtype_osd_gpui::view::{HEIGHT, Osd, WIDTH};

const BOTTOM_MARGIN: f32 = 44.0;
const POLL: Duration = Duration::from_millis(30);

/// Same rule as voxtype's `Config::runtime_dir` (macOS has no XDG_RUNTIME_DIR).
fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("voxtype")
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

#[cfg(target_os = "linux")]
fn placement(_cx: &App) -> (Bounds<gpui::Pixels>, WindowKind) {
    use gpui::layer_shell::{Anchor, KeyboardInteractivity, Layer, LayerShellOptions};
    let bounds = Bounds::new(point(px(0.), px(0.)), size(px(WIDTH), px(HEIGHT)));
    let kind = WindowKind::LayerShell(LayerShellOptions {
        namespace: "voxtype-osd".into(),
        layer: Layer::Overlay,
        anchor: Anchor::BOTTOM,
        margin: Some((px(0.), px(0.), px(BOTTOM_MARGIN), px(0.))),
        keyboard_interactivity: KeyboardInteractivity::None,
        ..Default::default()
    });
    (bounds, kind)
}

/// Bottom centre of the main display, above the Dock. PopUp is a
/// non-activating panel, so showing it never takes focus from the app being
/// dictated into.
#[cfg(target_os = "macos")]
fn placement(cx: &App) -> (Bounds<gpui::Pixels>, WindowKind) {
    let area = cx
        .primary_display()
        .map(|d| d.visible_bounds())
        .unwrap_or_else(|| Bounds::new(point(px(0.), px(0.)), size(px(1440.), px(900.))));
    let origin = point(
        area.center().x - px(WIDTH / 2.0),
        area.bottom() - px(HEIGHT + BOTTOM_MARGIN),
    );
    (Bounds::new(origin, size(px(WIDTH), px(HEIGHT))), WindowKind::PopUp)
}

#[cfg(target_os = "linux")]
fn click_through(window: &mut Window) {
    window.set_input_region(Some(&[]));
}

/// GPUI has no click-through on macOS, and an NSPanel hides itself whenever
/// its app is inactive, which this one always is.
#[cfg(target_os = "macos")]
fn click_through(window: &mut Window) {
    use objc::runtime::{NO, Object, YES};
    use objc::{msg_send, sel, sel_impl};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(handle) = window.window_handle() else { return };
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else { return };
    unsafe {
        let view = appkit.ns_view.as_ptr() as *mut Object;
        let panel: *mut Object = msg_send![view, window];
        let _: () = msg_send![panel, setIgnoresMouseEvents: YES];
        let _: () = msg_send![panel, setHidesOnDeactivate: NO];
    }
}

fn open_overlay(cx: &mut App, feed: Arc<Mutex<Feed>>, state: Rc<Cell<Daemon>>, open: Rc<Cell<bool>>) {
    let (bounds, kind) = placement(cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: None,
        focus: false,
        kind,
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
        click_through(window);
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
            // No Dock icon or menu bar on macOS; no effect on Linux.
            cx.set_activation_policy(gpui::ActivationPolicy::Accessory);
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

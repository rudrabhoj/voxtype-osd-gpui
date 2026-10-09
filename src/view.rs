//! The overlay itself, drawn with GPUI primitives in Catppuccin Mocha.

use std::f32::consts::PI;

use gpui::{
    BoxShadow, ColorSpace, Context, FontWeight, Hsla, IntoElement, ParentElement, Render, Rgba,
    Styled, Window, canvas, div, fill, linear_color_stop, linear_gradient, point, px, size,
    Bounds,
};

use crate::presenter::{BARS, Snapshot};

pub const WIDTH: f32 = 340.0;
pub const HEIGHT: f32 = 56.0;

// Catppuccin Mocha.
const CRUST: u32 = 0x11111b;
const MANTLE: u32 = 0x181825;
const BASE: u32 = 0x1e1e2e;
const OVERLAY1: u32 = 0x7f849c;
const SUBTEXT1: u32 = 0xbac2de;
const TEXT: u32 = 0xcdd6f4;
const LAVENDER: u32 = 0xb4befe;
const BLUE: u32 = 0x89b4fa;
const SAPPHIRE: u32 = 0x74c7ec;
const RED: u32 = 0xf38ba8;

/// Blue is the theme: bars run through its Mocha neighbours, Blue in the middle.
/// Oldest bar on the left, newest (next to the timer) on the right.
const RAMP: [u32; 3] = [SAPPHIRE, BLUE, LAVENDER];

const MIC: &str = "\u{f036c}";
const FONT: &str = "Maple Mono NF";

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn linear_to_srgb(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

fn to_oklab(hex: u32) -> [f32; 3] {
    let ch = |shift: u32| srgb_to_linear(((hex >> shift) & 0xff) as f32 / 255.0);
    let (r, g, b) = (ch(16), ch(8), ch(0));
    let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
    let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
    let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
    [
        0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
        1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
        0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
    ]
}

fn from_oklab([l, a, b]: [f32; 3], alpha: f32) -> Hsla {
    let l_ = (l + 0.3963377774 * a + 0.2158037573 * b).powi(3);
    let m_ = (l - 0.1055613458 * a - 0.0638541728 * b).powi(3);
    let s_ = (l - 0.0894841775 * a - 1.2914855480 * b).powi(3);
    Rgba {
        r: linear_to_srgb(4.0767416621 * l_ - 3.3077115913 * m_ + 0.2309699292 * s_),
        g: linear_to_srgb(-1.2684380046 * l_ + 2.6097574011 * m_ - 0.3413193965 * s_),
        b: linear_to_srgb(-0.0041960863 * l_ - 0.7034186147 * m_ + 1.7076147010 * s_),
        a: alpha.clamp(0.0, 1.0),
    }
    .into()
}

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

fn color(hex: u32, alpha: f32) -> Hsla {
    from_oklab(to_oklab(hex), alpha)
}

fn mix(a: u32, b: u32, t: f32, alpha: f32) -> Hsla {
    from_oklab(lerp3(to_oklab(a), to_oklab(b), t), alpha)
}

/// Sample the accent ramp at `p` in 0..1 (wrapping when `cyclic`).
fn ramp(p: f32, cyclic: bool) -> [f32; 3] {
    let n = RAMP.len();
    let (seg, t) = if cyclic {
        let x = p.rem_euclid(1.0) * n as f32;
        (x.floor() as usize % n, x.fract())
    } else {
        let x = p.clamp(0.0, 1.0) * (n - 1) as f32;
        ((x.floor() as usize).min(n - 2), x - x.floor().min((n - 2) as f32))
    };
    lerp3(to_oklab(RAMP[seg]), to_oklab(RAMP[(seg + 1) % n]), t)
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn glow(c: Hsla, blur: f32) -> BoxShadow {
    BoxShadow {
        color: c,
        offset: point(px(0.), px(0.)),
        blur_radius: px(blur),
        spread_radius: px(0.),
        inset: false,
    }
}

fn inset(c: Hsla, dy: f32, blur: f32, spread: f32) -> BoxShadow {
    BoxShadow {
        color: c,
        offset: point(px(0.), px(dy)),
        blur_radius: px(blur),
        spread_radius: px(spread),
        inset: true,
    }
}

pub struct Osd {
    source: Box<dyn FnMut() -> Option<Snapshot>>,
}

impl Osd {
    pub fn new(source: impl FnMut() -> Option<Snapshot> + 'static) -> Self {
        Self { source: Box::new(source) }
    }
}

impl Render for Osd {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        match (self.source)() {
            Some(s) => {
                window.request_animation_frame();
                pill(s).into_any_element()
            }
            None => {
                window.remove_window();
                div().into_any_element()
            }
        }
    }
}

fn pill(s: Snapshot) -> impl IntoElement {
    let level = s.level * s.appear;
    let m = s.transcribing;
    div()
        .size_full()
        .rounded_full()
        .opacity(s.alpha)
        .bg(linear_gradient(
            180.,
            linear_color_stop(color(BASE, 0.94), 0.),
            linear_color_stop(color(MANTLE, 0.95), 1.),
        )
        .color_space(ColorSpace::Oklab))
        // niri's active-border Blue, thin and translucent.
        .border_1()
        .border_color(color(BLUE, 0.45))
        .shadow(vec![
            // Glass edge: light along the top, shade along the bottom.
            inset(color(TEXT, 0.11), 1.0, 1.0, 0.0),
            inset(color(CRUST, 0.35), -1.0, 2.0, 0.0),
            // Voice-reactive inner glow.
            // Kept to a thin band at the rim so the body stays Base.
            inset(color(BLUE, 0.10 + 0.40 * level + 0.15 * m), 0.0, 7.0, -1.0),
        ])
        .flex()
        .items_center()
        .pl(px(10.))
        .pr(px(18.))
        .gap(px(12.))
        .child(orb(&s, level))
        .child(waveform(s.clone()))
        .child(clock(&s))
}

fn orb(s: &Snapshot, level: f32) -> impl IntoElement {
    let m = s.transcribing;
    let spin = s.t * (20.0 + 220.0 * m);
    let breath = 0.5 + 0.5 * (s.t * 2.4).sin();
    div()
        .size(px(36.))
        .flex_none()
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(linear_gradient(
            135. + spin,
            linear_color_stop(mix(BLUE, SAPPHIRE, m, 1.0), 0.),
            linear_color_stop(mix(LAVENDER, BLUE, m, 1.0), 1.),
        )
        .color_space(ColorSpace::Oklab))
        .shadow(vec![
            // The orb sits 10 px inside the pill's rounded end; a wider glow
            // would be cut square by the window edge.
            glow(
                color(BLUE, (0.30 + 0.45 * level + 0.15 * m * breath) * s.appear),
                3.0 + 5.0 * level.max(m * breath),
            ),
            inset(color(TEXT, 0.35), 1.0, 1.0, 0.0),
            inset(color(CRUST, 0.25), -2.0, 6.0, 0.0),
        ])
        .child(
            div()
                .font_family(FONT)
                .text_size(px(19.))
                .text_color(color(BASE, 1.0))
                .child(MIC),
        )
}

fn waveform(s: Snapshot) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds: Bounds<gpui::Pixels>, (), window, _| {
            let w = f32::from(bounds.size.width);
            let h = f32::from(bounds.size.height);
            let left = f32::from(bounds.origin.x);
            let mid = f32::from(bounds.origin.y) + h / 2.0;
            let pitch = w / BARS as f32;
            let bar_w = (pitch * 0.52).max(2.0);
            let max_h = h * 0.70;
            let m = s.transcribing;

            let mut quads = Vec::with_capacity(BARS);
            for (i, &v_rec) in s.bars.iter().enumerate() {
                let p = i as f32 / (BARS - 1) as f32;
                let cx = left + w - ((BARS - 1 - i) as f32 + s.scroll + 0.5) * pitch;
                let x = (cx - left) / w;
                let wave = 0.5 + 0.5 * (s.t * 5.2 - i as f32 * 0.42).sin();
                let v_trans = (0.12 + 0.42 * wave) * (PI * p).sin().powf(0.6);
                let v = (v_rec + (v_trans - v_rec) * m) * s.appear;
                let bar_h = bar_w + (max_h - bar_w) * v;

                let accent = lerp3(ramp(p, false), ramp(p - s.t * 0.35, true), m);
                let tint = lerp3(to_oklab(OVERLAY1), accent, smoothstep(0.04, 0.32, v));
                let top_accent = lerp3(ramp(p + 0.25, false), ramp(p + 0.25 - s.t * 0.35, true), m);
                let top_tint = lerp3(to_oklab(OVERLAY1), top_accent, smoothstep(0.04, 0.32, v));
                let edge = smoothstep(0.0, 0.22, x) * smoothstep(1.0, 0.96, x);
                let alpha = (0.55 + 0.45 * smoothstep(0.02, 0.45, v)) * edge;
                if alpha <= 0.01 {
                    continue;
                }
                let r = Bounds::new(
                    point(px(cx - bar_w / 2.0), px(mid - bar_h / 2.0)),
                    size(px(bar_w), px(bar_h)),
                );
                if v > 0.12 {
                    window.paint_drop_shadows(
                        r,
                        px(bar_w / 2.0).into(),
                        &[glow(from_oklab(tint, 0.8 * v * alpha), 4.0 + 9.0 * v)],
                    );
                }
                let top = from_oklab(top_tint, alpha);
                quads.push(
                    fill(
                        r,
                        linear_gradient(
                            180.,
                            linear_color_stop(top, 0.),
                            linear_color_stop(from_oklab(tint, alpha), 1.),
                        )
                        .color_space(ColorSpace::Oklab),
                    )
                    .corner_radii(px(bar_w / 2.0)),
                );
            }
            // Glows first, so one bar's halo never sits on top of its neighbour.
            for q in quads {
                window.paint_quad(q);
            }
        },
    )
    .flex_1()
    .h_full()
}

fn clock(s: &Snapshot) -> impl IntoElement {
    let m = s.transcribing;
    let pulse = 0.55 + 0.45 * (0.5 + 0.5 * (s.t * (4.0 + 3.0 * m)).sin());
    let secs = s.elapsed.as_secs();
    let dot = if m < 0.5 { RED } else { BLUE };
    let swap = (1.0 - 2.0 * m).abs();
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(8.))
        .child(
            div()
                .size(px(7.))
                .rounded_full()
                // Fade one colour out and the other in; mixing Red into Blue
                // would pass through off-palette shades.
                .bg(color(dot, pulse * swap))
                .shadow(vec![glow(color(dot, 0.6 * pulse * swap), 6.0)]),
        )
        .child(
            div()
                .font_family(FONT)
                .font_weight(FontWeight::MEDIUM)
                .text_size(px(13.))
                .text_color(color(SUBTEXT1, 1.0))
                .child(format!("{}:{:02}", secs / 60, secs % 60)),
        )
}

//! Screen Ruler tool — on-screen pixel measurement.
//!
//! UX (matches PowerToys Screen Ruler's measure + bounds modes):
//! - Hotkey activates a fullscreen overlay with a faint crosshair
//!   following the cursor on the active monitor.
//! - Click + drag (no modifier) → line from press point to cursor; the
//!   pixel distance is shown live next to the cursor.
//! - Click + drag with **Ctrl** held → rectangle instead of a line; the
//!   label shows `W X H`.
//! - Click + drag with **Shift** held → line snaps to the nearest
//!   enabled angle (cardinals / diagonals / thirds / octants, each
//!   group configurable in the GUI).
//! - Release → the measurement persists on screen until the next click
//!   or Esc.
//! - Click again → starts a new measurement at the click point.
//! - Esc → exit.
//!
//! Single-monitor only: if the cursor leaves the active output during a
//! drag, the measurement is cancelled. Cross-output measurement is not
//! supported in v0.3.0.
//!
//! Additional keys:
//! - **Backspace** — undo the last measurement (or the in-progress one).
//! - **M** — toggle a magnifier loupe near the cursor that shows a
//!   zoomed-in view of the captured screen, for pixel-precise placement.

use std::io;

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_keyboard, delegate_layer, delegate_output,
    delegate_pointer, delegate_registry, delegate_seat, delegate_shm,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers},
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
    },
    shell::{
        WaylandSurface,
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler,
            LayerSurface, LayerSurfaceConfigure,
        },
    },
    shm::{Shm, ShmHandler, slot::SlotPool},
};
use wayland_client::{
    Connection, QueueHandle,
    globals::registry_queue_init,
    protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
};

use crate::{capture, font};

// Constants we don't expose for config — internal layout / alpha values
// that shouldn't need user fiddling.
const LINE_ALPHA: u8 = 230;
const LABEL_BG_ALPHA: u8 = 210;
const FONT_SCALE: u32 = 2;
const LABEL_PAD_X: i32 = 6;
const LABEL_PAD_Y: i32 = 4;
const LABEL_OFFSET: i32 = 16;
const MAG_SOURCE_SIZE: i32 = 15; // odd so there's a centre pixel
const MAG_OFFSET: i32 = 24;

// Defaults mirror gui/src/config.rs::Config::default() — fallback if the
// per-field cosmic-config files don't exist.
const DEFAULT_LINE_THICKNESS: u32 = 2;
const DEFAULT_CROSSHAIR_ALPHA: u8 = 90;
const DEFAULT_MAG_ZOOM: u32 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineStyle {
    Solid,
    Dotted,
    Dashed,
}

impl LineStyle {
    fn parse(s: &str) -> Self {
        match s.trim().trim_matches('"') {
            "dotted" => Self::Dotted,
            "dashed" => Self::Dashed,
            _ => Self::Solid,
        }
    }
    /// Visibility for the i-th step along the line. Patterns are short
    /// enough to read at the smallest practical line lengths but long
    /// enough that the gaps are clearly visible at 1× display scale.
    fn visible(self, step: i32) -> bool {
        match self {
            Self::Solid => true,
            Self::Dotted => (step.rem_euclid(8)) < 4, // 4 on, 4 off
            Self::Dashed => (step.rem_euclid(16)) < 10, // 10 on, 6 off
        }
    }
}

#[derive(Debug, Clone)]
struct RulerConfig {
    line_thickness: i32,
    line_rgb: (u8, u8, u8),
    crosshair_alpha: u8,
    mag_zoom: i32,
    magnifier_default: bool,
    line_style: LineStyle,
    /// Allowed snap angles in degrees from horizontal, expanded from the
    /// four GUI groups. Empty means Shift has no effect on the line.
    snap_angles_deg: Vec<f32>,
}

impl RulerConfig {
    fn load() -> Self {
        Self {
            line_thickness: read_u32("screen_ruler_line_thickness_px")
                .unwrap_or(DEFAULT_LINE_THICKNESS) as i32,
            line_rgb: read_color("screen_ruler_line_color")
                .unwrap_or((255, 255, 255)),
            crosshair_alpha: read_u32("screen_ruler_crosshair_alpha")
                .unwrap_or(DEFAULT_CROSSHAIR_ALPHA as u32) as u8,
            mag_zoom: read_u32("screen_ruler_magnifier_zoom")
                .unwrap_or(DEFAULT_MAG_ZOOM) as i32,
            magnifier_default: read_bool("screen_ruler_magnifier_default")
                .unwrap_or(false),
            line_style: read_string("screen_ruler_line_style")
                .map(|s| LineStyle::parse(&s))
                .unwrap_or(LineStyle::Solid),
            snap_angles_deg: collect_snap_angles(),
        }
    }
}

/// Expand the four GUI snap-group toggles into a concrete angle list.
/// Defaults match `Config::default()` in the GUI (cardinals + diagonals).
fn collect_snap_angles() -> Vec<f32> {
    let cardinals = read_bool("screen_ruler_snap_cardinals").unwrap_or(true);
    let diagonals = read_bool("screen_ruler_snap_diagonals").unwrap_or(true);
    let thirds = read_bool("screen_ruler_snap_thirds").unwrap_or(false);
    let octants = read_bool("screen_ruler_snap_octants").unwrap_or(false);
    let mut out = Vec::new();
    if cardinals {
        out.push(0.0);
        out.push(90.0);
    }
    if diagonals {
        out.push(45.0);
    }
    if thirds {
        out.push(30.0);
        out.push(60.0);
    }
    if octants {
        out.push(15.0);
        out.push(75.0);
    }
    out
}

fn read_string(field: &str) -> Option<String> {
    let path = config_path(field);
    std::fs::read_to_string(path).ok()
}

/// Read a cosmic-config field as u32 (also covers u8 — caller clamps).
fn read_u32(field: &str) -> Option<u32> {
    let path = config_path(field);
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// Read a string cosmic-config field expecting `#RRGGBB` hex.
fn read_color(field: &str) -> Option<(u8, u8, u8)> {
    let path = config_path(field);
    let raw = std::fs::read_to_string(path).ok()?;
    let s = raw.trim().trim_matches('"').trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some((r, g, b))
}

fn read_bool(field: &str) -> Option<bool> {
    let path = config_path(field);
    let raw = std::fs::read_to_string(path).ok()?;
    raw.trim().parse().ok()
}

fn config_path(field: &str) -> std::path::PathBuf {
    let xdg_config = std::env::var("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_default();
            std::path::PathBuf::from(home).join(".config")
        });
    xdg_config
        .join("cosmic")
        .join("com.pyxyll.CosmicToys")
        .join("v1")
        .join(field)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Line,
    Rect,
}

#[derive(Debug, Clone, Copy)]
struct Measurement {
    /// Index into `state.outputs` — the surface where the click started.
    output_idx: usize,
    start: (f32, f32),
    end: (f32, f32),
    mode: Mode,
    /// True while the user is still holding the button; false once
    /// released (measurement persists).
    dragging: bool,
}

pub fn show() -> io::Result<()> {
    // Capture the screen ONCE up front so the magnifier loupe has
    // something to zoom into. The capture is frozen for the lifetime of
    // the overlay — fine for measuring static UI elements (which is the
    // whole point of the ruler).
    let image = capture::screenshot()?;

    let conn = Connection::connect_to_env().map_err(io::Error::other)?;
    let (globals, mut event_queue) =
        registry_queue_init(&conn).map_err(io::Error::other)?;
    let qh: QueueHandle<State> = event_queue.handle();

    let compositor = CompositorState::bind(&globals, &qh).map_err(io::Error::other)?;
    let layer_shell = LayerShell::bind(&globals, &qh).map_err(io::Error::other)?;
    let shm = Shm::bind(&globals, &qh).map_err(io::Error::other)?;
    let pool = SlotPool::new(64 * 1024 * 1024, &shm).map_err(io::Error::other)?;

    let config = RulerConfig::load();
    let mut state = State {
        registry_state: RegistryState::new(&globals),
        seat_state: SeatState::new(&globals, &qh),
        output_state: OutputState::new(&globals, &qh),
        compositor,
        layer_shell,
        shm,
        pool,
        outputs: Vec::new(),
        keyboard: None,
        pointer: None,
        shift_held: false,
        ctrl_held: false,
        measurements: Vec::new(),
        dragging_idx: None,
        image,
        magnifier_on: config.magnifier_default,
        config,
        exit: false,
    };

    event_queue
        .roundtrip(&mut state)
        .map_err(io::Error::other)?;

    while !state.exit {
        event_queue
            .blocking_dispatch(&mut state)
            .map_err(io::Error::other)?;
    }

    Ok(())
}

struct OutputSurface {
    wl_output: wl_output::WlOutput,
    layer: LayerSurface,
    surface: wl_surface::WlSurface,
    /// Compositor-space top-left of this output. Used to map surface-
    /// local cursor coords back into the captured image for the loupe.
    pos: (i32, i32),
    size: (u32, u32),
    configured: bool,
    /// Surface-local cursor position, set/updated on pointer Enter/Motion.
    /// None means the cursor isn't on this output.
    cursor: Option<(f32, f32)>,
    /// State changed since last render and a redraw is wanted. The next
    /// frame callback picks it up.
    needs_redraw: bool,
    /// True between rendering a frame and receiving its frame callback.
    /// While true, motion events just flip `needs_redraw` and don't
    /// render — keeps us from out-allocating the SlotPool on fast mouse
    /// drags (same bug the picker hit pre-fix).
    frame_pending: bool,
}

struct State {
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    compositor: CompositorState,
    layer_shell: LayerShell,
    shm: Shm,
    pool: SlotPool,
    outputs: Vec<OutputSurface>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    pointer: Option<wl_pointer::WlPointer>,
    /// Live modifier state from the most recent
    /// keyboard.update_modifiers event. Ctrl picks Rect mode at click
    /// time; Shift enables angle-snap during a Line drag.
    shift_held: bool,
    ctrl_held: bool,
    /// All measurements on screen, in drawing order (older first). New
    /// click appends; Backspace pops the last; Esc exits (state drops).
    measurements: Vec<Measurement>,
    /// Index into `measurements` of the one currently being dragged, if
    /// any. `None` means no active drag.
    dragging_idx: Option<usize>,
    /// Frozen screen capture, used as the source for the magnifier loupe.
    image: image::RgbaImage,
    /// Toggled by M; shows a zoom-in loupe near the cursor for pixel-
    /// precise placement of line / rect endpoints.
    magnifier_on: bool,
    /// Cached at startup from cosmic-config. Visual knobs that the GUI
    /// exposes as sliders / inputs.
    config: RulerConfig,
    exit: bool,
}

impl State {
    fn add_output(&mut self, qh: &QueueHandle<Self>, wl_output: wl_output::WlOutput) {
        let Some(info) = self.output_state.info(&wl_output) else {
            return;
        };
        let pos = info.logical_position.unwrap_or((0, 0));
        let size = info
            .logical_size
            .map(|(w, h)| (w as u32, h as u32))
            .unwrap_or((0, 0));

        let surface = self.compositor.create_surface(qh);
        let layer = self.layer_shell.create_layer_surface(
            qh,
            surface.clone(),
            Layer::Overlay,
            Some("cosmic-toys-screen-ruler"),
            Some(&wl_output),
        );
        layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer.set_exclusive_zone(-1);
        // Exclusive keyboard so Esc + Shift modifier tracking work. The
        // user dismisses with click or Esc, so grabbing focus is fine.
        layer.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
        layer.commit();

        self.outputs.push(OutputSurface {
            wl_output,
            layer,
            surface,
            pos,
            size,
            configured: false,
            cursor: None,
            needs_redraw: false,
            frame_pending: false,
        });
    }

    /// Mark this output dirty. If no frame is currently in flight, render
    /// immediately and start awaiting a frame callback; otherwise the
    /// existing pending frame's callback will pick up the dirty flag.
    fn request_redraw(&mut self, idx: usize, qh: &QueueHandle<Self>) {
        self.outputs[idx].needs_redraw = true;
        if !self.outputs[idx].frame_pending && self.outputs[idx].configured {
            self.draw_output(idx, qh);
        }
    }

    fn draw_output(&mut self, idx: usize, qh: &QueueHandle<Self>) {
        let out = &self.outputs[idx];
        let (sw, sh) = out.size;
        if sw == 0 || sh == 0 {
            return;
        }

        let stride = sw as i32 * 4;
        let (buf, canvas) = match self.pool.create_buffer(
            sw as i32,
            sh as i32,
            stride,
            wl_shm::Format::Argb8888,
        ) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("screen_ruler: buffer alloc failed: {e}");
                return;
            }
        };

        // Fully transparent background — the overlay only shows the
        // crosshair + measurement.
        for px in canvas.chunks_exact_mut(4) {
            px[0] = 0;
            px[1] = 0;
            px[2] = 0;
            px[3] = 0;
        }

        // Are we currently dragging on this surface? Affects whether we
        // draw the crosshair (suppressed during an active drag — the
        // line + endpoints carry the visual).
        let dragging_here = self
            .dragging_idx
            .and_then(|i| self.measurements.get(i))
            .map(|m| m.output_idx == idx)
            .unwrap_or(false);

        let cfg = self.config.clone();

        // Layer 1: faint crosshair through the cursor (only on the output
        // that has the cursor, and only when not actively dragging here).
        if let Some((cx, cy)) = out.cursor
            && !dragging_here
        {
            draw_crosshair(canvas, sw, sh, cx as i32, cy as i32, cfg.crosshair_alpha);
        }

        // Layer 2: every measurement that lives on this output, in order.
        // Layer 3: a label per measurement so the user can read every
        // value at once even with many on screen.
        for m in self.measurements.iter().filter(|m| m.output_idx == idx) {
            draw_measurement(canvas, sw, sh, m, &cfg);
            let label = format_label(m);
            draw_label(canvas, sw, sh, m.end.0 as i32, m.end.1 as i32, &label);
        }

        // Layer 4 (optional): magnifier loupe near the cursor.
        if self.magnifier_on
            && let Some((cx, cy)) = out.cursor
        {
            draw_magnifier(
                canvas,
                sw,
                sh,
                &self.image,
                out.pos,
                cx as i32,
                cy as i32,
                cfg.mag_zoom,
            );
        }

        out.surface.damage_buffer(0, 0, sw as i32, sh as i32);
        // Request the next frame callback so subsequent motion-driven
        // redraws have a chance to land. The callback is where we
        // discover the compositor consumed the buffer.
        out.surface.frame(qh, out.surface.clone());
        let _ = buf.attach_to(&out.surface);
        out.surface.commit();

        let out = &mut self.outputs[idx];
        out.needs_redraw = false;
        out.frame_pending = true;
    }
}

// =============================================================================
// Pixel-painting helpers
// =============================================================================

/// Premultiplied ARGB8888 (BGRA byte order). For white with alpha=a, each
/// channel is `a`; for black with alpha=a, only the A byte is `a`.
fn put_pixel(canvas: &mut [u8], cw: u32, ch: u32, x: i32, y: i32, color: [u8; 4]) {
    if x < 0 || y < 0 || x >= cw as i32 || y >= ch as i32 {
        return;
    }
    let di = ((y * cw as i32 + x) * 4) as usize;
    canvas[di] = color[0];
    canvas[di + 1] = color[1];
    canvas[di + 2] = color[2];
    canvas[di + 3] = color[3];
}

fn fill_rect(canvas: &mut [u8], cw: u32, ch: u32, x: i32, y: i32, w: i32, h: i32, color: [u8; 4]) {
    for dy in 0..h {
        for dx in 0..w {
            put_pixel(canvas, cw, ch, x + dx, y + dy, color);
        }
    }
}

/// Bresenham's line algorithm, drawing a 1px line then padding to the
/// desired thickness with parallel offset stamps. `style` gates each
/// step's stamp — solid stamps every step, dotted/dashed stamp on/off
/// per their patterns (see LineStyle::visible).
fn draw_line_thick(
    canvas: &mut [u8],
    cw: u32,
    ch: u32,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    thickness: i32,
    color: [u8; 4],
    style: LineStyle,
) {
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    let mut x = x0;
    let mut y = y0;
    let half = thickness / 2;
    let mut step: i32 = 0;
    loop {
        if style.visible(step) {
            // Stamp a thickness × thickness block centred on each point.
            for tx in -half..=(thickness - half - 1) {
                for ty in -half..=(thickness - half - 1) {
                    put_pixel(canvas, cw, ch, x + tx, y + ty, color);
                }
            }
        }
        if x == x1 && y == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
        step += 1;
    }
}

fn draw_crosshair(canvas: &mut [u8], cw: u32, ch: u32, cx: i32, cy: i32, alpha: u8) {
    // Premultiplied white at the requested alpha — RGB = A.
    let c = [alpha, alpha, alpha, alpha];
    // Horizontal line across the full surface width
    for x in 0..cw as i32 {
        put_pixel(canvas, cw, ch, x, cy, c);
    }
    // Vertical line across the full surface height
    for y in 0..ch as i32 {
        put_pixel(canvas, cw, ch, cx, y, c);
    }
}

fn draw_measurement(canvas: &mut [u8], cw: u32, ch: u32, m: &Measurement, cfg: &RulerConfig) {
    // Premultiplied: each channel = color_channel * alpha / 255.
    let a = LINE_ALPHA as u32;
    let (r, g, b) = cfg.line_rgb;
    let line_color = [
        ((b as u32 * a) / 255) as u8,
        ((g as u32 * a) / 255) as u8,
        ((r as u32 * a) / 255) as u8,
        LINE_ALPHA,
    ];
    let sx = m.start.0 as i32;
    let sy = m.start.1 as i32;
    let ex = m.end.0 as i32;
    let ey = m.end.1 as i32;
    let t = cfg.line_thickness;

    let style = cfg.line_style;
    match m.mode {
        Mode::Line => {
            draw_line_thick(canvas, cw, ch, sx, sy, ex, ey, t, line_color, style);
            // Endpoint dots scale gently with the thickness. They're
            // always solid so the start / end of a dotted line still
            // reads as anchored.
            let dot = (t + 2).max(4);
            let half = dot / 2;
            fill_rect(canvas, cw, ch, sx - half, sy - half, dot, dot, line_color);
            fill_rect(canvas, cw, ch, ex - half, ey - half, dot, dot, line_color);
        }
        Mode::Rect => {
            let rx = sx.min(ex);
            let ry = sy.min(ey);
            let rw = (ex - sx).abs();
            let rh = (ey - sy).abs();
            draw_line_thick(canvas, cw, ch, rx, ry, rx + rw, ry, t, line_color, style);
            draw_line_thick(canvas, cw, ch, rx, ry + rh, rx + rw, ry + rh, t, line_color, style);
            draw_line_thick(canvas, cw, ch, rx, ry, rx, ry + rh, t, line_color, style);
            draw_line_thick(canvas, cw, ch, rx + rw, ry, rx + rw, ry + rh, t, line_color, style);
        }
    }
}

/// Project `end` onto the line from `start` whose angle matches the
/// nearest entry in `allowed_angles_deg`, preserving the drag's length.
/// Each base angle in [0, 90] expands to its four (-180, 180] reflections
/// (a, -a, 180-a, a-180) so the snap works in any quadrant.
fn snap_to_angle(
    start: (f32, f32),
    end: (f32, f32),
    allowed_angles_deg: &[f32],
) -> (f32, f32) {
    let dx = end.0 - start.0;
    let dy = end.1 - start.1;
    let dist = (dx * dx + dy * dy).sqrt();
    if dist < 0.5 || allowed_angles_deg.is_empty() {
        return end;
    }
    let drag_deg = dy.atan2(dx).to_degrees();

    let mut best_angle = drag_deg;
    let mut best_diff = f32::INFINITY;
    for &a in allowed_angles_deg {
        for candidate in [a, -a, 180.0 - a, a - 180.0] {
            let d = shortest_angle_diff(candidate, drag_deg).abs();
            if d < best_diff {
                best_diff = d;
                best_angle = candidate;
            }
        }
    }
    let rad = best_angle.to_radians();
    (start.0 + rad.cos() * dist, start.1 + rad.sin() * dist)
}

/// Signed shortest difference between two degree angles, in (-180, 180].
fn shortest_angle_diff(a: f32, b: f32) -> f32 {
    let mut d = (a - b).rem_euclid(360.0);
    if d > 180.0 {
        d -= 360.0;
    }
    d
}

fn format_label(m: &Measurement) -> String {
    let dx = m.end.0 - m.start.0;
    let dy = m.end.1 - m.start.1;
    match m.mode {
        Mode::Line => {
            let dist = (dx * dx + dy * dy).sqrt().round() as u32;
            format!("{dist}")
        }
        Mode::Rect => {
            let w = dx.abs().round() as u32;
            let h = dy.abs().round() as u32;
            format!("{w} X {h}")
        }
    }
}

/// Magnifier loupe: small zoomed view of the captured screen anchored
/// near the cursor, with a 1-pixel reticle highlighting the targeted
/// source pixel. Position auto-flips so the loupe never falls off the
/// edge of the output.
fn draw_magnifier(
    canvas: &mut [u8],
    cw: u32,
    ch: u32,
    image: &image::RgbaImage,
    out_pos: (i32, i32),
    cursor_x: i32,
    cursor_y: i32,
    mag_scale: i32,
) {
    let mag_dest_size = MAG_SOURCE_SIZE * mag_scale;

    // Pick a corner: top-right by default; flip when off-surface.
    let mut mx = cursor_x + MAG_OFFSET;
    let mut my = cursor_y - MAG_OFFSET - mag_dest_size;
    if mx + mag_dest_size + 4 > cw as i32 {
        mx = cursor_x - MAG_OFFSET - mag_dest_size;
    }
    if my < 2 {
        my = cursor_y + MAG_OFFSET;
    }

    // 2px white border, 1px black inner border (contrast against light bg).
    let white = [255, 255, 255, 255];
    let black = [0, 0, 0, 255];
    fill_rect(canvas, cw, ch, mx - 2, my - 2, mag_dest_size + 4, 2, white);
    fill_rect(canvas, cw, ch, mx - 2, my + mag_dest_size, mag_dest_size + 4, 2, white);
    fill_rect(canvas, cw, ch, mx - 2, my, 2, mag_dest_size, white);
    fill_rect(canvas, cw, ch, mx + mag_dest_size, my, 2, mag_dest_size, white);
    fill_rect(canvas, cw, ch, mx, my, mag_dest_size, 1, black);
    fill_rect(canvas, cw, ch, mx, my + mag_dest_size - 1, mag_dest_size, 1, black);
    fill_rect(canvas, cw, ch, mx, my, 1, mag_dest_size, black);
    fill_rect(canvas, cw, ch, mx + mag_dest_size - 1, my, 1, mag_dest_size, black);

    // Sample MAG_SOURCE_SIZE pixels centred on the compositor-space
    // cursor; stamp each as a mag_scale × mag_scale block.
    let img_w = image.width() as i32;
    let img_h = image.height() as i32;
    let centre = MAG_SOURCE_SIZE / 2;
    let src_origin_x = out_pos.0 + cursor_x - centre;
    let src_origin_y = out_pos.1 + cursor_y - centre;

    for sy in 0..MAG_SOURCE_SIZE {
        for sx in 0..MAG_SOURCE_SIZE {
            let src_x = src_origin_x + sx;
            let src_y = src_origin_y + sy;
            let sample = if src_x >= 0 && src_y >= 0 && src_x < img_w && src_y < img_h {
                let p = image.get_pixel(src_x as u32, src_y as u32);
                [p[2], p[1], p[0], 255]
            } else {
                [0, 0, 0, 255]
            };
            let dx0 = mx + sx * mag_scale;
            let dy0 = my + sy * mag_scale;
            for ddy in 0..mag_scale {
                for ddx in 0..mag_scale {
                    put_pixel(canvas, cw, ch, dx0 + ddx, dy0 + ddy, sample);
                }
            }
        }
    }

    // Reticle: 1px box outlining the centre source pixel.
    let bx = mx + centre * mag_scale;
    let by = my + centre * mag_scale;
    for i in 0..mag_scale {
        put_pixel(canvas, cw, ch, bx + i, by, white);
        put_pixel(canvas, cw, ch, bx + i, by + mag_scale - 1, white);
        put_pixel(canvas, cw, ch, bx, by + i, white);
        put_pixel(canvas, cw, ch, bx + mag_scale - 1, by + i, white);
    }
}

fn draw_label(canvas: &mut [u8], cw: u32, ch: u32, anchor_x: i32, anchor_y: i32, text: &str) {
    let tw = font::text_width(text, FONT_SCALE) as i32;
    let th = font::text_height(FONT_SCALE) as i32;
    // Position bottom-right of the anchor (cursor) with a small offset.
    // Clamp so we never run off the surface.
    let mut x = anchor_x + LABEL_OFFSET;
    let mut y = anchor_y + LABEL_OFFSET;
    let box_w = tw + LABEL_PAD_X * 2;
    let box_h = th + LABEL_PAD_Y * 2;
    if x + box_w >= cw as i32 {
        x = anchor_x - box_w - LABEL_OFFSET;
    }
    if y + box_h >= ch as i32 {
        y = anchor_y - box_h - LABEL_OFFSET;
    }

    // Dark semi-transparent background for legibility on any wallpaper.
    fill_rect(
        canvas,
        cw,
        ch,
        x,
        y,
        box_w,
        box_h,
        [0, 0, 0, LABEL_BG_ALPHA],
    );

    // White text.
    font::draw_text(
        canvas,
        cw,
        ch,
        x + LABEL_PAD_X,
        y + LABEL_PAD_Y,
        text,
        [255, 255, 255],
        FONT_SCALE,
    );
}

// =============================================================================
// SCTK handler impls
// =============================================================================

impl CompositorHandler for State {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: i32,
    ) {
    }
    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }
    fn frame(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        _: u32,
    ) {
        if let Some(idx) = self.outputs.iter().position(|o| &o.surface == surface) {
            self.outputs[idx].frame_pending = false;
            if self.outputs[idx].needs_redraw {
                self.draw_output(idx, qh);
            }
        }
    }
    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for State {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        wl_output: wl_output::WlOutput,
    ) {
        self.add_output(qh, wl_output);
    }
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        wl_output: wl_output::WlOutput,
    ) {
        self.outputs.retain(|o| o.wl_output != wl_output);
    }
}

impl SeatHandler for State {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }
    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        match capability {
            Capability::Keyboard if self.keyboard.is_none() => {
                self.keyboard = self.seat_state.get_keyboard(qh, &seat, None).ok();
            }
            Capability::Pointer if self.pointer.is_none() => {
                self.pointer = self.seat_state.get_pointer(qh, &seat).ok();
            }
            _ => {}
        }
    }
    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        capability: Capability,
    ) {
        match capability {
            Capability::Keyboard => {
                if let Some(k) = self.keyboard.take() {
                    k.release();
                }
            }
            Capability::Pointer => {
                if let Some(p) = self.pointer.take() {
                    p.release();
                }
            }
            _ => {}
        }
    }
    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl KeyboardHandler for State {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: &wl_surface::WlSurface,
        _: u32,
        _: &[u32],
        _: &[Keysym],
    ) {
    }
    fn leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: &wl_surface::WlSurface,
        _: u32,
    ) {
    }
    fn press_key(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        match event.keysym {
            Keysym::Escape => {
                self.exit = true;
            }
            // Quick undo: pop the most recent measurement (the one being
            // dragged, if any, or otherwise the last persisted).
            Keysym::BackSpace => {
                if let Some(m) = self.measurements.pop() {
                    self.dragging_idx = None;
                    let idx = m.output_idx;
                    self.request_redraw(idx, qh);
                }
            }
            // M toggles the magnifier loupe near the cursor.
            Keysym::m => {
                self.magnifier_on = !self.magnifier_on;
                for i in 0..self.outputs.len() {
                    self.request_redraw(i, qh);
                }
            }
            _ => {}
        }
    }
    fn release_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        _: KeyEvent,
    ) {
    }
    fn update_modifiers(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        modifiers: Modifiers,
        _: u32,
    ) {
        self.shift_held = modifiers.shift;
        self.ctrl_held = modifiers.ctrl;
    }
}

impl PointerHandler for State {
    fn pointer_frame(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        _: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        let mut to_redraw: Vec<usize> = Vec::new();

        for event in events {
            let Some(idx) = self
                .outputs
                .iter()
                .position(|o| o.surface == event.surface)
            else {
                continue;
            };
            match event.kind {
                PointerEventKind::Enter { .. } | PointerEventKind::Motion { .. } => {
                    let pos = (event.position.0 as f32, event.position.1 as f32);
                    self.outputs[idx].cursor = Some(pos);
                    // If we're dragging on THIS output, update endpoint.
                    // If we're dragging on a DIFFERENT output, the cursor
                    // wandered off mid-drag — cancel just the in-flight
                    // measurement (others persist), redraw both outputs.
                    if let Some(d) = self.dragging_idx {
                        let drag_on = self.measurements[d].output_idx;
                        if drag_on == idx {
                            // Snap is live: re-evaluated on every motion
                            // event so the user can engage/disengage it
                            // mid-drag with Shift.
                            let start = self.measurements[d].start;
                            let mode = self.measurements[d].mode;
                            let end = if mode == Mode::Line
                                && self.shift_held
                                && !self.config.snap_angles_deg.is_empty()
                            {
                                snap_to_angle(start, pos, &self.config.snap_angles_deg)
                            } else {
                                pos
                            };
                            self.measurements[d].end = end;
                        } else {
                            self.measurements.remove(d);
                            self.dragging_idx = None;
                            to_redraw.push(drag_on);
                        }
                    }
                    to_redraw.push(idx);
                }
                PointerEventKind::Leave { .. } => {
                    self.outputs[idx].cursor = None;
                    to_redraw.push(idx);
                }
                PointerEventKind::Press { button, .. } => {
                    // Left button only (BTN_LEFT in linux/input-event-codes.h = 0x110).
                    if button != 0x110 {
                        continue;
                    }
                    let pos = self.outputs[idx].cursor.unwrap_or((
                        event.position.0 as f32,
                        event.position.1 as f32,
                    ));
                    let mode = if self.ctrl_held { Mode::Rect } else { Mode::Line };
                    // Append a new measurement and mark it as the active
                    // drag. Older measurements stay.
                    self.measurements.push(Measurement {
                        output_idx: idx,
                        start: pos,
                        end: pos,
                        mode,
                        dragging: true,
                    });
                    self.dragging_idx = Some(self.measurements.len() - 1);
                    to_redraw.push(idx);
                }
                PointerEventKind::Release { button, .. } => {
                    if button != 0x110 {
                        continue;
                    }
                    if let Some(d) = self.dragging_idx {
                        self.measurements[d].dragging = false;
                        self.dragging_idx = None;
                        to_redraw.push(self.measurements[d].output_idx);
                    }
                }
                _ => {}
            }
        }

        // De-dup and dispatch redraws via the rate-limiter so fast motion
        // doesn't out-allocate the SlotPool.
        to_redraw.sort_unstable();
        to_redraw.dedup();
        for idx in to_redraw {
            self.request_redraw(idx, qh);
        }
    }
}

impl LayerShellHandler for State {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {}
    fn configure(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        layer: &LayerSurface,
        config: LayerSurfaceConfigure,
        _: u32,
    ) {
        let Some(idx) = self.outputs.iter().position(|o| &o.layer == layer) else {
            return;
        };
        let (w, h) = config.new_size;
        if w != 0 && h != 0 {
            self.outputs[idx].size = (w, h);
        }
        self.outputs[idx].configured = true;
        self.request_redraw(idx, qh);
    }
}

impl ShmHandler for State {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for State {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

delegate_compositor!(State);
delegate_keyboard!(State);
delegate_layer!(State);
delegate_output!(State);
delegate_pointer!(State);
delegate_registry!(State);
delegate_seat!(State);
delegate_shm!(State);

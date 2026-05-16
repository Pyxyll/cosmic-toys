//! OCR ("Live Text") tool — capture the screen, run tesseract over it,
//! show every recognized word as a hover-selectable bbox, copy the text
//! the user click-drags through.
//!
//! Runtime dep: `tesseract` + `tesseract-data-<lang>` from the user's
//! distro. We check for the binary up front and exit with a friendly
//! error if it's missing.
//!
//! Flow:
//! 1. capture::screenshot() → in-memory RGBA (full compositor space).
//! 2. PNG temp file + spawn `tesseract <file> stdout -l eng tsv`.
//! 3. Parse TSV → `Vec<Word>` in reading order.
//! 4. Open a fullscreen layer-shell overlay per output. Each word's
//!    bbox is rendered at three intensities:
//!    - default (faint underline) so users can see what's recognised
//!    - hover (outline around the word at the cursor)
//!    - selected (filled with a semi-transparent highlight)
//! 5. Click + drag through words to extend selection in reading order.
//! 6. Release with non-empty selection → wl-copy the concatenated text
//!    (space within a line, newline between lines) and exit.
//! 7. Esc cancels without copying.

use std::io::{self, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Instant;

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

use crate::capture;

const CONF_FLOOR: f32 = 30.0;

// Visual tiers for each word's bbox.
const UNDERLINE_ALPHA: u8 = 50;
const HOVER_OUTLINE_ALPHA: u8 = 140;
/// Blue-ish text-selection tint (RGB 59/130/246), alpha tuned so the
/// underlying text still reads through clearly.
const HIGHLIGHT_RGB: (u8, u8, u8) = (59, 130, 246);
const HIGHLIGHT_ALPHA: u8 = 110;
/// Vertical padding added above + below each highlighted line so the
/// fill reads as a continuous strip rather than tight per-word boxes.
const HIGHLIGHT_VPAD: i32 = 2;

#[derive(Debug, Clone)]
pub struct Word {
    /// Compositor-space bbox.
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub text: String,
    /// Grouping keys from tesseract — used both for sorting into
    /// reading order and for choosing separators when assembling the
    /// final selected text.
    pub block: u32,
    pub par: u32,
    pub line: u32,
    pub word_num: u32,
}

pub fn show() -> io::Result<()> {
    if Command::new("tesseract").arg("--version").output().is_err() {
        eprintln!(
            "cosmic-toysd: ocr: `tesseract` not found on PATH. \
             Install with `sudo pacman -S tesseract tesseract-data-eng` \
             (or your distro's equivalent)."
        );
        return Err(io::Error::other("tesseract missing"));
    }

    let started = Instant::now();
    let image = capture::screenshot()?;
    eprintln!(
        "cosmic-toysd: ocr: captured {}x{} in {} ms",
        image.width(),
        image.height(),
        started.elapsed().as_millis()
    );

    let mut words = run_tesseract(&image)?;
    // Reading order — sort defensively even though tesseract usually
    // emits in roughly the right order.
    words.sort_by_key(|w| (w.block, w.par, w.line, w.word_num, w.x));
    eprintln!("cosmic-toysd: ocr: recognized {} words", words.len());

    // Now the SCTK overlay.
    let conn = Connection::connect_to_env().map_err(io::Error::other)?;
    let (globals, mut event_queue) =
        registry_queue_init(&conn).map_err(io::Error::other)?;
    let qh: QueueHandle<State> = event_queue.handle();

    let compositor = CompositorState::bind(&globals, &qh).map_err(io::Error::other)?;
    let layer_shell = LayerShell::bind(&globals, &qh).map_err(io::Error::other)?;
    let shm = Shm::bind(&globals, &qh).map_err(io::Error::other)?;
    let pool = SlotPool::new(64 * 1024 * 1024, &shm).map_err(io::Error::other)?;

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
        words,
        hovered_idx: None,
        anchor_idx: None,
        cursor_idx: None,
        dragging: false,
        copy_on_exit: false,
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

    if state.copy_on_exit {
        let text = build_selected_text(&state.words, state.anchor_idx, state.cursor_idx);
        if !text.is_empty() {
            deliver_copy(&text);
        }
    }
    Ok(())
}

// =============================================================================
// OCR pipeline
// =============================================================================

fn run_tesseract(image: &image::RgbaImage) -> io::Result<Vec<Word>> {
    let tmp = png_tempfile(image)?;
    let started = Instant::now();
    let output = Command::new("tesseract")
        .arg(&tmp)
        .arg("stdout")
        .arg("-l")
        .arg("eng")
        .arg("tsv")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| io::Error::other(format!("spawn tesseract: {e}")))?;
    let _ = std::fs::remove_file(&tmp);
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(io::Error::other(format!(
            "tesseract failed: {}: {}",
            output.status,
            stderr.lines().last().unwrap_or("")
        )));
    }
    eprintln!(
        "cosmic-toysd: ocr: tesseract returned in {} ms",
        started.elapsed().as_millis()
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(parse_tsv(&stdout))
}

fn png_tempfile(image: &image::RgbaImage) -> io::Result<PathBuf> {
    let path = std::env::temp_dir().join(format!(
        "cosmic-toys-ocr-{}-{}.png",
        std::process::id(),
        Instant::now().elapsed().as_nanos()
    ));
    let mut bytes: Vec<u8> = Vec::new();
    image
        .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png)
        .map_err(|e| io::Error::other(format!("encode png: {e}")))?;
    let mut f = std::fs::File::create(&path)?;
    f.write_all(&bytes)?;
    Ok(path)
}

fn parse_tsv(stdout: &str) -> Vec<Word> {
    let mut out = Vec::new();
    for line in stdout.lines().skip(1) {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 12 {
            continue;
        }
        let level = f[0].parse::<u32>().unwrap_or(0);
        if level != 5 {
            continue;
        }
        let conf = f[10].parse::<f32>().unwrap_or(-1.0);
        if conf < CONF_FLOOR {
            continue;
        }
        let text = f[11].trim();
        if text.is_empty() {
            continue;
        }
        out.push(Word {
            block: f[2].parse().unwrap_or(0),
            par: f[3].parse().unwrap_or(0),
            line: f[4].parse().unwrap_or(0),
            word_num: f[5].parse().unwrap_or(0),
            x: f[6].parse().unwrap_or(0),
            y: f[7].parse().unwrap_or(0),
            w: f[8].parse().unwrap_or(0),
            h: f[9].parse().unwrap_or(0),
            text: text.to_string(),
        });
    }
    out
}

// =============================================================================
// Selection helpers
// =============================================================================

/// Inclusive [min(anchor, cursor), max(...)] range over `words` if both
/// endpoints are set, else None.
fn selected_range(
    anchor: Option<usize>,
    cursor: Option<usize>,
) -> Option<std::ops::RangeInclusive<usize>> {
    match (anchor, cursor) {
        (Some(a), Some(c)) => Some(a.min(c)..=a.max(c)),
        _ => None,
    }
}

/// Concatenate selected words with line-aware separators: space between
/// words on the same line, newline between lines / paragraphs / blocks.
fn build_selected_text(
    words: &[Word],
    anchor: Option<usize>,
    cursor: Option<usize>,
) -> String {
    let Some(range) = selected_range(anchor, cursor) else {
        return String::new();
    };
    let mut out = String::new();
    let mut prev: Option<&Word> = None;
    for i in range {
        let Some(w) = words.get(i) else { continue };
        if let Some(p) = prev {
            if w.block != p.block || w.par != p.par || w.line != p.line {
                out.push('\n');
            } else {
                out.push(' ');
            }
        }
        out.push_str(&w.text);
        prev = Some(w);
    }
    out
}

fn deliver_copy(text: &str) {
    if let Ok(mut child) = Command::new("wl-copy").stdin(Stdio::piped()).spawn()
        && let Some(mut stdin) = child.stdin.take()
    {
        let _ = stdin.write_all(text.as_bytes());
        drop(stdin);
        let _ = child.wait();
    }
    // Best-effort notification with the first 60 chars as preview.
    let mut preview = text.replace('\n', " ");
    if preview.chars().count() > 60 {
        preview = preview.chars().take(60).collect::<String>() + "…";
    }
    let _ = Command::new("notify-send")
        .arg("cosmic-toys")
        .arg(format!("Copied: {preview}"))
        .status();
}

// =============================================================================
// SCTK state + rendering
// =============================================================================

struct OutputSurface {
    wl_output: wl_output::WlOutput,
    layer: LayerSurface,
    surface: wl_surface::WlSurface,
    /// Compositor-space top-left of this output.
    pos: (i32, i32),
    size: (u32, u32),
    configured: bool,
    cursor: Option<(f32, f32)>,
    needs_redraw: bool,
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
    /// All recognised words, in reading order. Index = the selection
    /// addressing space for anchor / cursor.
    words: Vec<Word>,
    hovered_idx: Option<usize>,
    anchor_idx: Option<usize>,
    cursor_idx: Option<usize>,
    dragging: bool,
    /// Set on a successful Release with a non-empty selection. The main
    /// loop reads this after exit to decide whether to wl-copy.
    copy_on_exit: bool,
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
            Some("cosmic-toys-ocr"),
            Some(&wl_output),
        );
        layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer.set_exclusive_zone(-1);
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

    fn request_redraw(&mut self, idx: usize, qh: &QueueHandle<Self>) {
        self.outputs[idx].needs_redraw = true;
        if !self.outputs[idx].frame_pending && self.outputs[idx].configured {
            self.draw_output(idx, qh);
        }
    }

    fn redraw_all(&mut self, qh: &QueueHandle<Self>) {
        for i in 0..self.outputs.len() {
            self.request_redraw(i, qh);
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
                eprintln!("ocr: buffer alloc failed: {e}");
                return;
            }
        };

        // Start fully transparent; we only draw word bboxes on top.
        for px in canvas.chunks_exact_mut(4) {
            px[0] = 0;
            px[1] = 0;
            px[2] = 0;
            px[3] = 0;
        }

        let out_pos = out.pos;
        let selected = selected_range(self.anchor_idx, self.cursor_idx);
        let hovered = self.hovered_idx;

        // First pass: paint the selection as one continuous strip per
        // line. Real text selections aren't per-word boxes; grouping by
        // (block, par, line) and filling the enclosing rect gives the
        // familiar "highlight bar" look.
        if let Some(range) = selected.as_ref() {
            let mut by_line: std::collections::BTreeMap<(u32, u32, u32), (i32, i32, i32, i32)> =
                std::collections::BTreeMap::new();
            for i in range.clone() {
                let Some(w) = self.words.get(i) else { continue };
                let key = (w.block, w.par, w.line);
                let entry = by_line
                    .entry(key)
                    .or_insert((w.x, w.y, w.x + w.w, w.y + w.h));
                entry.0 = entry.0.min(w.x);
                entry.1 = entry.1.min(w.y);
                entry.2 = entry.2.max(w.x + w.w);
                entry.3 = entry.3.max(w.y + w.h);
            }
            let (r, g, b) = HIGHLIGHT_RGB;
            let a = HIGHLIGHT_ALPHA as u32;
            let highlight = [
                ((b as u32 * a) / 255) as u8, // B (premultiplied)
                ((g as u32 * a) / 255) as u8, // G
                ((r as u32 * a) / 255) as u8, // R
                HIGHLIGHT_ALPHA,
            ];
            for (_, (x0, y0, x1, y1)) in by_line {
                let sx = x0 - out_pos.0;
                let sy = y0 - out_pos.1 - HIGHLIGHT_VPAD;
                let rw = x1 - x0;
                let rh = (y1 - y0) + HIGHLIGHT_VPAD * 2;
                if sx + rw <= 0 || sy + rh <= 0 || sx >= sw as i32 || sy >= sh as i32 {
                    continue;
                }
                fill_rect(canvas, sw, sh, sx, sy, rw, rh, highlight);
            }
        }

        // Second pass: every word still gets the discoverable underline
        // unless it's already inside the selection strip. Hovered word
        // (when not selected) gets the outline.
        for (i, w) in self.words.iter().enumerate() {
            let sx0 = w.x - out_pos.0;
            let sy0 = w.y - out_pos.1;
            if sx0 + w.w <= 0
                || sy0 + w.h <= 0
                || sx0 >= sw as i32
                || sy0 >= sh as i32
            {
                continue;
            }

            let is_selected = selected.as_ref().is_some_and(|r| r.contains(&i));
            if is_selected {
                continue; // already covered by the line-strip fill above
            }
            if hovered == Some(i) {
                outline_rect(canvas, sw, sh, sx0, sy0, w.w, w.h,
                    [HOVER_OUTLINE_ALPHA, HOVER_OUTLINE_ALPHA, HOVER_OUTLINE_ALPHA, HOVER_OUTLINE_ALPHA]);
            } else {
                let c = [UNDERLINE_ALPHA, UNDERLINE_ALPHA, UNDERLINE_ALPHA, UNDERLINE_ALPHA];
                fill_rect(canvas, sw, sh, sx0, sy0 + w.h - 1, w.w, 1, c);
            }
        }

        out.surface.damage_buffer(0, 0, sw as i32, sh as i32);
        out.surface.frame(qh, out.surface.clone());
        let _ = buf.attach_to(&out.surface);
        out.surface.commit();

        let out = &mut self.outputs[idx];
        out.needs_redraw = false;
        out.frame_pending = true;
    }

    /// Hit-test the word list at compositor-space (x, y). Returns the
    /// index of the first word whose bbox contains the point, if any.
    fn hit(&self, cx: i32, cy: i32) -> Option<usize> {
        self.words.iter().position(|w| {
            cx >= w.x && cx < w.x + w.w && cy >= w.y && cy < w.y + w.h
        })
    }
}

// =============================================================================
// Pixel helpers
// =============================================================================

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

fn outline_rect(canvas: &mut [u8], cw: u32, ch: u32, x: i32, y: i32, w: i32, h: i32, color: [u8; 4]) {
    if w <= 0 || h <= 0 {
        return;
    }
    fill_rect(canvas, cw, ch, x, y, w, 1, color);
    fill_rect(canvas, cw, ch, x, y + h - 1, w, 1, color);
    fill_rect(canvas, cw, ch, x, y, 1, h, color);
    fill_rect(canvas, cw, ch, x + w - 1, y, 1, h, color);
}

// =============================================================================
// SCTK handler impls
// =============================================================================

impl CompositorHandler for State {
    fn scale_factor_changed(
        &mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: i32,
    ) {}
    fn transform_changed(
        &mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: wl_output::Transform,
    ) {}
    fn frame(
        &mut self, _: &Connection, qh: &QueueHandle<Self>, surface: &wl_surface::WlSurface, _: u32,
    ) {
        if let Some(idx) = self.outputs.iter().position(|o| &o.surface == surface) {
            self.outputs[idx].frame_pending = false;
            if self.outputs[idx].needs_redraw {
                self.draw_output(idx, qh);
            }
        }
    }
    fn surface_enter(
        &mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput,
    ) {}
    fn surface_leave(
        &mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput,
    ) {}
}

impl OutputHandler for State {
    fn output_state(&mut self) -> &mut OutputState { &mut self.output_state }
    fn new_output(&mut self, _: &Connection, qh: &QueueHandle<Self>, wl_output: wl_output::WlOutput) {
        self.add_output(qh, wl_output);
    }
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, wl_output: wl_output::WlOutput) {
        self.outputs.retain(|o| o.wl_output != wl_output);
    }
}

impl SeatHandler for State {
    fn seat_state(&mut self) -> &mut SeatState { &mut self.seat_state }
    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
    fn new_capability(
        &mut self, _: &Connection, qh: &QueueHandle<Self>, seat: wl_seat::WlSeat, capability: Capability,
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
        &mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat, capability: Capability,
    ) {
        match capability {
            Capability::Keyboard => { if let Some(k) = self.keyboard.take() { k.release(); } }
            Capability::Pointer => { if let Some(p) = self.pointer.take() { p.release(); } }
            _ => {}
        }
    }
    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl KeyboardHandler for State {
    fn enter(
        &mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard,
        _: &wl_surface::WlSurface, _: u32, _: &[u32], _: &[Keysym],
    ) {}
    fn leave(
        &mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard,
        _: &wl_surface::WlSurface, _: u32,
    ) {}
    fn press_key(
        &mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard,
        _: u32, event: KeyEvent,
    ) {
        if event.keysym == Keysym::Escape {
            // Esc bails without copying — even if a selection was made.
            self.copy_on_exit = false;
            self.exit = true;
        }
    }
    fn release_key(
        &mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard,
        _: u32, _: KeyEvent,
    ) {}
    fn update_modifiers(
        &mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard,
        _: u32, _: Modifiers, _: u32,
    ) {}
}

impl PointerHandler for State {
    fn pointer_frame(
        &mut self, _: &Connection, qh: &QueueHandle<Self>, _: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        let mut needs_full_redraw = false;
        for event in events {
            let Some(idx) = self
                .outputs
                .iter()
                .position(|o| o.surface == event.surface)
            else {
                continue;
            };
            let surface_pos = (event.position.0 as f32, event.position.1 as f32);
            let out_pos = self.outputs[idx].pos;
            let comp_x = out_pos.0 + surface_pos.0 as i32;
            let comp_y = out_pos.1 + surface_pos.1 as i32;

            match event.kind {
                PointerEventKind::Enter { .. } | PointerEventKind::Motion { .. } => {
                    self.outputs[idx].cursor = Some(surface_pos);
                    let new_hover = self.hit(comp_x, comp_y);
                    if new_hover != self.hovered_idx {
                        self.hovered_idx = new_hover;
                        needs_full_redraw = true;
                    }
                    if self.dragging && let Some(h) = new_hover && self.cursor_idx != Some(h) {
                        self.cursor_idx = Some(h);
                        needs_full_redraw = true;
                    }
                }
                PointerEventKind::Leave { .. } => {
                    self.outputs[idx].cursor = None;
                    if self.hovered_idx.is_some() {
                        self.hovered_idx = None;
                        needs_full_redraw = true;
                    }
                }
                PointerEventKind::Press { button, .. } => {
                    if button != 0x110 {
                        continue;
                    }
                    // Start a new selection. If the click missed every
                    // word, clear any previous selection but don't start
                    // a drag (nothing to extend from).
                    if let Some(h) = self.hit(comp_x, comp_y) {
                        self.anchor_idx = Some(h);
                        self.cursor_idx = Some(h);
                        self.dragging = true;
                    } else {
                        self.anchor_idx = None;
                        self.cursor_idx = None;
                        self.dragging = false;
                    }
                    needs_full_redraw = true;
                }
                PointerEventKind::Release { button, .. } => {
                    if button != 0x110 {
                        continue;
                    }
                    self.dragging = false;
                    // Auto-copy if there's a non-empty selection.
                    if self.anchor_idx.is_some() && self.cursor_idx.is_some() {
                        self.copy_on_exit = true;
                        self.exit = true;
                    }
                }
                _ => {}
            }
        }
        if needs_full_redraw {
            self.redraw_all(qh);
        }
    }
}

impl LayerShellHandler for State {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {}
    fn configure(
        &mut self, _: &Connection, qh: &QueueHandle<Self>, layer: &LayerSurface,
        config: LayerSurfaceConfigure, _: u32,
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
    fn shm_state(&mut self) -> &mut Shm { &mut self.shm }
}

impl ProvidesRegistryState for State {
    fn registry(&mut self) -> &mut RegistryState { &mut self.registry_state }
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

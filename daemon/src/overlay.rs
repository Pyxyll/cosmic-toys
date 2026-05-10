//! Fullscreen layer-shell overlay.
//!
//! One [`LayerSurface`] per output: each surface is anchored to its output,
//! configured to that output's size, and renders the slice of the captured
//! image that corresponds to its position in compositor space. Pointer events
//! are surface-relative; we add the output's logical position to derive the
//! capture-space pixel for picking.

use std::io;

use crate::{capture, font};

/// Capture the screen, open the picker overlay, and return the picked hex
/// (or `None` if the user cancelled).
pub fn pick_color() -> io::Result<Option<String>> {
    let img = capture::screenshot()?;
    run(img)
}

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
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
    },
    shm::{Shm, ShmHandler, slot::SlotPool},
};
use wayland_client::{
    Connection, QueueHandle,
    globals::registry_queue_init,
    protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
};

pub fn run(image: image::RgbaImage) -> io::Result<Option<String>> {
    let conn = Connection::connect_to_env().map_err(io::Error::other)?;
    let (globals, mut event_queue) = registry_queue_init(&conn).map_err(io::Error::other)?;
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
        pointer_focus: None,
        image,
        result: None,
        exit: false,
    };

    while !state.exit {
        event_queue
            .blocking_dispatch(&mut state)
            .map_err(io::Error::other)?;
    }

    Ok(state.result)
}

struct OutputSurface {
    wl_output: wl_output::WlOutput,
    layer: LayerSurface,
    surface: wl_surface::WlSurface,
    /// Compositor-space top-left position of this output.
    pos: (i32, i32),
    /// Logical (compositor) size of this output, used as the surface size.
    size: (u32, u32),
    configured: bool,
    /// Latest cursor position in surface-local coords. None means cursor is
    /// not over this output and the magnifier should not be drawn here.
    cursor: Option<(i32, i32)>,
    /// Set when state changed and a redraw is wanted. The next frame callback
    /// will pick it up. Prevents redrawing faster than the compositor can
    /// consume buffers (which exhausts the SlotPool on rapid motion).
    needs_redraw: bool,
    /// True between rendering a frame and receiving its frame callback.
    /// While true, motion events only flip needs_redraw and don't render.
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
    pointer_focus: Option<wl_surface::WlSurface>,
    image: image::RgbaImage,
    result: Option<String>,
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
            Some("cosmic-toys"),
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

    /// Marks the given output dirty. If no frame is currently in flight for
    /// it, render immediately and start awaiting a frame callback. Otherwise
    /// the existing pending frame's callback will pick up the dirty flag.
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
        let (ox, oy) = out.pos;
        let stride = sw as i32 * 4;
        let (buf, canvas) = match self
            .pool
            .create_buffer(sw as i32, sh as i32, stride, wl_shm::Format::Argb8888)
        {
            Ok(v) => v,
            Err(e) => {
                eprintln!("color-picker: buffer alloc failed: {e}");
                return;
            }
        };

        // Source slice in the captured image is (ox, oy) — (ox+sw, oy+sh).
        // Both should be in capture pixels at 1:1 with logical compositor
        // coordinates for COSMIC at 100% scale.
        let img_w = self.image.width();
        let img_h = self.image.height();
        let src = self.image.as_raw();

        for y in 0..sh {
            for x in 0..sw {
                let sx = (ox + x as i32).clamp(0, img_w as i32 - 1) as u32;
                let sy = (oy + y as i32).clamp(0, img_h as i32 - 1) as u32;
                let si = ((sy * img_w + sx) * 4) as usize;
                let di = ((y * sw + x) * 4) as usize;
                // RGBA → ARGB8888 (little-endian: BGRA byte order)
                canvas[di] = src[si + 2];
                canvas[di + 1] = src[si + 1];
                canvas[di + 2] = src[si];
                canvas[di + 3] = 0xFF;
            }
        }

        if let Some((cx, cy)) = out.cursor {
            draw_magnifier(canvas, sw, sh, cx, cy, &self.image, ox, oy);
            // Hex readout label below the magnifier.
            let img_w = self.image.width() as i32;
            let img_h = self.image.height() as i32;
            let sx = (ox + cx).clamp(0, img_w - 1);
            let sy = (oy + cy).clamp(0, img_h - 1);
            let p = self.image.get_pixel(sx as u32, sy as u32);
            let hex = format!("#{:02X}{:02X}{:02X}", p[0], p[1], p[2]);
            draw_label(canvas, sw, sh, cx, cy + 110, &hex, [p[0], p[1], p[2]]);
        }

        out.surface.damage_buffer(0, 0, sw as i32, sh as i32);
        out.surface.frame(qh, out.surface.clone());
        let _ = buf.attach_to(&out.surface);
        out.surface.commit();

        let out = &mut self.outputs[idx];
        out.needs_redraw = false;
        out.frame_pending = true;
    }

    fn pick_at(&mut self, surface: &wl_surface::WlSurface, x: f64, y: f64) {
        let Some(out) = self.outputs.iter().find(|o| &o.surface == surface) else {
            return;
        };
        let img_w = self.image.width() as i32;
        let img_h = self.image.height() as i32;
        let sx = (out.pos.0 + x as i32).clamp(0, img_w - 1) as u32;
        let sy = (out.pos.1 + y as i32).clamp(0, img_h - 1) as u32;
        let p = self.image.get_pixel(sx, sy);
        self.result = Some(format!("#{:02X}{:02X}{:02X}", p[0], p[1], p[2]));
        self.exit = true;
    }
}

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
        if let Some(i) = self.outputs.iter().position(|o| &o.surface == surface) {
            self.outputs[i].frame_pending = false;
            if self.outputs[i].needs_redraw {
                self.draw_output(i, qh);
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
                    k.release()
                }
            }
            Capability::Pointer => {
                if let Some(p) = self.pointer.take() {
                    p.release()
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
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        if event.keysym == Keysym::Escape {
            self.exit = true;
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
        _: Modifiers,
        _: u32,
    ) {
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
        for e in events {
            let idx = self.outputs.iter().position(|o| o.surface == e.surface);
            match e.kind {
                PointerEventKind::Enter { .. } => {
                    self.pointer_focus = Some(e.surface.clone());
                    if let Some(i) = idx {
                        self.outputs[i].cursor = Some((e.position.0 as i32, e.position.1 as i32));
                        // Clear seed magnifiers on all *other* outputs — once
                        // the cursor is on a real one, the others should go
                        // quiet so we're not showing two lenses.
                        let others: Vec<usize> = (0..self.outputs.len()).filter(|j| *j != i).collect();
                        for j in others {
                            if self.outputs[j].cursor.is_some() {
                                self.outputs[j].cursor = None;
                                self.request_redraw(j, qh);
                            }
                        }
                        self.request_redraw(i, qh);
                    }
                }
                PointerEventKind::Leave { .. } => {
                    if self.pointer_focus.as_ref() == Some(&e.surface) {
                        self.pointer_focus = None;
                    }
                    if let Some(i) = idx {
                        self.outputs[i].cursor = None;
                        self.request_redraw(i, qh);
                    }
                }
                PointerEventKind::Motion { .. } => {
                    if let Some(i) = idx {
                        self.outputs[i].cursor = Some((e.position.0 as i32, e.position.1 as i32));
                        self.request_redraw(i, qh);
                    }
                }
                PointerEventKind::Press { button: 272, .. } => {
                    self.pick_at(&e.surface, e.position.0, e.position.1);
                }
                PointerEventKind::Press { button: 273, .. } => {
                    self.exit = true;
                }
                _ => {}
            }
        }
    }
}

impl LayerShellHandler for State {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {
        self.exit = true;
    }
    fn configure(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _: u32,
    ) {
        let Some(idx) = self.outputs.iter().position(|o| &o.layer == layer) else {
            return;
        };
        let (w, h) = configure.new_size;
        if w > 0 && h > 0 {
            self.outputs[idx].size = (w, h);
        }
        self.outputs[idx].configured = true;
        // Note: we don't seed a cursor position here. Cosmic doesn't fire
        // Pointer.Enter for a freshly-created layer surface until the user
        // nudges the mouse, so the magnifier appears on first motion rather
        // than immediately. Seeding it at centre looked broken on multi-monitor
        // setups (a stuck lens on the inactive output), so we accept the small
        // delay until first motion in exchange for a cleaner appearance.
        self.draw_output(idx, qh);
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

/// Draws a zoom magnifier centred at `(cx, cy)` on the canvas. The lens shows
/// pixels from `image` (in capture-space) around the cursor, zoomed `ZOOM`x.
/// A white border ring frames it; a black reticle marks the exact pixel.
fn draw_magnifier(
    canvas: &mut [u8],
    cw: u32,
    cy_canvas_h: u32,
    cx: i32,
    cy: i32,
    image: &image::RgbaImage,
    img_ox: i32,
    img_oy: i32,
) {
    const RADIUS: i32 = 90;
    const ZOOM: i32 = 8;
    const RING_THICKNESS: i32 = 3;
    const RETICLE_HALF: i32 = 6;
    const RETICLE_GAP: i32 = 2;

    let img_w = image.width() as i32;
    let img_h = image.height() as i32;
    let src = image.as_raw();
    let cw_i = cw as i32;
    let ch_i = cy_canvas_h as i32;

    let r2 = RADIUS * RADIUS;
    let ring_inner = (RADIUS - RING_THICKNESS) * (RADIUS - RING_THICKNESS);

    for dy in -RADIUS..=RADIUS {
        let py = cy + dy;
        if py < 0 || py >= ch_i {
            continue;
        }
        for dx in -RADIUS..=RADIUS {
            let px = cx + dx;
            if px < 0 || px >= cw_i {
                continue;
            }
            let dist_sq = dx * dx + dy * dy;
            if dist_sq > r2 {
                continue;
            }

            let di = ((py * cw_i + px) * 4) as usize;

            // White border ring around the lens.
            if dist_sq > ring_inner {
                canvas[di] = 0xFF;
                canvas[di + 1] = 0xFF;
                canvas[di + 2] = 0xFF;
                canvas[di + 3] = 0xFF;
                continue;
            }

            // Black reticle: a thin cross with a small gap at the centre, so
            // the picked pixel itself stays visible.
            let on_h = dy == 0 && (RETICLE_GAP..=RETICLE_HALF).contains(&dx.abs());
            let on_v = dx == 0 && (RETICLE_GAP..=RETICLE_HALF).contains(&dy.abs());
            if on_h || on_v {
                canvas[di] = 0x00;
                canvas[di + 1] = 0x00;
                canvas[di + 2] = 0x00;
                canvas[di + 3] = 0xFF;
                continue;
            }

            // Zoomed sample: source pixel = (cursor in image coords) + (dx/zoom, dy/zoom).
            let sx = (img_ox + cx + dx / ZOOM).clamp(0, img_w - 1);
            let sy = (img_oy + cy + dy / ZOOM).clamp(0, img_h - 1);
            let si = ((sy * img_w + sx) * 4) as usize;
            canvas[di] = src[si + 2];
            canvas[di + 1] = src[si + 1];
            canvas[di + 2] = src[si];
            canvas[di + 3] = 0xFF;
        }
    }
}

/// Renders the hex readout label centred horizontally at `(cx, top_y)`.
/// Layout: dark rounded background pill, a colour swatch on the left, then
/// the white hex text.
fn draw_label(
    canvas: &mut [u8],
    cw: u32,
    ch: u32,
    cx: i32,
    top_y: i32,
    hex: &str,
    swatch_color: [u8; 3],
) {
    // `font` is a sibling sub-module declared at the top of this file.

    const SCALE: u32 = 3;
    const PAD: i32 = 8;
    const SWATCH_GAP: i32 = 8;

    let text_w = font::text_width(hex, SCALE) as i32;
    let text_h = font::text_height(SCALE) as i32;
    let swatch_size = text_h;

    let inner_w = swatch_size + SWATCH_GAP + text_w;
    let bg_w = inner_w + PAD * 2;
    let bg_h = text_h + PAD * 2;
    let bg_x = cx - bg_w / 2;
    let bg_y = top_y;

    let cw_i = cw as i32;
    let ch_i = ch as i32;

    // Dark, slightly translucent background pill.
    for y in 0..bg_h {
        for x in 0..bg_w {
            let px = bg_x + x;
            let py = bg_y + y;
            if px < 0 || px >= cw_i || py < 0 || py >= ch_i {
                continue;
            }
            let di = ((py * cw_i + px) * 4) as usize;
            // Soft rounded corners by skipping the very corner pixels.
            let in_corner = (x < 4 || x >= bg_w - 4) && (y < 4 || y >= bg_h - 4);
            let cx_close = (x.min(bg_w - 1 - x)) as f32 - 4.0;
            let cy_close = (y.min(bg_h - 1 - y)) as f32 - 4.0;
            if in_corner && (cx_close * cx_close + cy_close * cy_close).sqrt() > 4.0 {
                continue;
            }
            // Blend ~85% dark over the underlying pixel.
            canvas[di] = (canvas[di] as f32 * 0.15 + 25.0) as u8;
            canvas[di + 1] = (canvas[di + 1] as f32 * 0.15 + 25.0) as u8;
            canvas[di + 2] = (canvas[di + 2] as f32 * 0.15 + 25.0) as u8;
            canvas[di + 3] = 0xFF;
        }
    }

    // Colour swatch on the left.
    let sw_x = bg_x + PAD;
    let sw_y = bg_y + PAD;
    for y in 0..swatch_size {
        for x in 0..swatch_size {
            let px = sw_x + x;
            let py = sw_y + y;
            if px < 0 || px >= cw_i || py < 0 || py >= ch_i {
                continue;
            }
            let di = ((py * cw_i + px) * 4) as usize;
            canvas[di] = swatch_color[2];
            canvas[di + 1] = swatch_color[1];
            canvas[di + 2] = swatch_color[0];
            canvas[di + 3] = 0xFF;
        }
    }

    // Hex text in white, to the right of the swatch.
    let text_x = sw_x + swatch_size + SWATCH_GAP;
    let text_y = sw_y;
    font::draw_text(canvas, cw, ch, text_x, text_y, hex, [0xFF, 0xFF, 0xFF], SCALE);
}

delegate_compositor!(State);
delegate_output!(State);
delegate_seat!(State);
delegate_keyboard!(State);
delegate_pointer!(State);
delegate_layer!(State);
delegate_shm!(State);
delegate_registry!(State);

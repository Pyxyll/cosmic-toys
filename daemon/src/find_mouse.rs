//! "Find Mouse" tool — dim overlay with a circular cutout that follows
//! the cursor on whichever monitor the cursor is on.
//!
//! Workflow:
//! - Hotkey fires `cosmic-toysd run find_mouse`. Layer surfaces are
//!   created across every output but rendered fully transparent — nothing
//!   visible yet.
//! - As soon as the user moves the mouse, the layer surface on the
//!   monitor receiving pointer-motion events activates: dim background +
//!   bright soft-edged cutout at the cursor. Other monitors stay
//!   untouched. The cutout follows motion in real time.
//! - User dismisses by clicking (the click is consumed by the overlay)
//!   or pressing Esc.
//!
//! No auto-timeout. Frame callbacks drive redraws on the active output;
//! inactive outputs sit idle until pointer motion brings the cursor onto
//! them.
//!
//! Known limitation: COSMIC doesn't fire `Pointer.Enter` for a freshly
//! created layer surface, so the user has to nudge the mouse for the
//! overlay to appear at all. Acceptable trade-off — they're trying to
//! find the cursor anyway, so they're going to nudge.

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

const SPOTLIGHT_RADIUS: f32 = 90.0;
const FEATHER: f32 = 28.0;
const DIM_ALPHA: u8 = 140;
/// Bright ring at the cutout boundary so the spotlight reads against
/// dark backgrounds. White, ~4px thick. Ring color + thickness should
/// move into Config in a follow-up so users can pick an accent color.
const RING_THICKNESS: f32 = 4.0;
const RING_ALPHA: u8 = 220;

pub fn show() -> io::Result<()> {
    let conn = Connection::connect_to_env().map_err(io::Error::other)?;
    let (globals, mut event_queue) =
        registry_queue_init(&conn).map_err(io::Error::other)?;
    let qh: QueueHandle<State> = event_queue.handle();

    let compositor = CompositorState::bind(&globals, &qh).map_err(io::Error::other)?;
    let layer_shell = LayerShell::bind(&globals, &qh).map_err(io::Error::other)?;
    let shm = Shm::bind(&globals, &qh).map_err(io::Error::other)?;
    // 64MB sized to comfortably hold three 4K outputs' worth of buffers.
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
        exit: false,
    };

    // First roundtrip: registry processed, outputs added, layer surfaces
    // created. Configure events arrive in subsequent dispatches and
    // trigger initial (transparent) draws.
    event_queue
        .roundtrip(&mut state)
        .map_err(io::Error::other)?;

    // Event-driven loop. blocking_dispatch wakes on every compositor
    // event: configures, frame callbacks, pointer events, key events.
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
    size: (u32, u32),
    configured: bool,
    /// Last known surface-local cursor position.
    cursor: Option<(f32, f32)>,
    /// True when this surface should render dim+spotlight. False until
    /// the pointer first enters/moves on it; flipped to false on other
    /// outputs whenever the cursor moves to a new output.
    active: bool,
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
    exit: bool,
}

impl State {
    fn add_output(&mut self, qh: &QueueHandle<Self>, wl_output: wl_output::WlOutput) {
        let Some(info) = self.output_state.info(&wl_output) else {
            return;
        };
        let size = info
            .logical_size
            .map(|(w, h)| (w as u32, h as u32))
            .unwrap_or((0, 0));

        let surface = self.compositor.create_surface(qh);
        let layer = self.layer_shell.create_layer_surface(
            qh,
            surface.clone(),
            Layer::Overlay,
            Some("cosmic-toys-find-mouse"),
            Some(&wl_output),
        );
        layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer.set_exclusive_zone(-1);
        // Need keyboard so Esc can dismiss; user can also click-to-dismiss.
        layer.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
        layer.commit();

        self.outputs.push(OutputSurface {
            wl_output,
            layer,
            surface,
            size,
            configured: false,
            cursor: None,
            active: false,
        });
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
                eprintln!("find_mouse: buffer alloc failed: {e}");
                return;
            }
        };

        if !out.active {
            // Inactive surface: fully transparent. Whatever the user has
            // on this monitor stays untouched.
            for px in canvas.chunks_exact_mut(4) {
                px[0] = 0;
                px[1] = 0;
                px[2] = 0;
                px[3] = 0;
            }
        } else {
            // Active surface: dim outside, bright ring at the cutout edge,
            // transparent inside. Premultiplied ARGB8888 — for black we
            // leave RGB channels at 0; for the white ring we set them to
            // the same value as alpha (premultiplied white).
            let (cx, cy) = out.cursor.unwrap_or((sw as f32 * 0.5, sh as f32 * 0.5));
            let inner = SPOTLIGHT_RADIUS;
            let ring_inner = inner - RING_THICKNESS;
            let outer = inner + FEATHER;
            let ring_inner2 = ring_inner * ring_inner;
            let inner2 = inner * inner;
            let outer2 = outer * outer;
            for y in 0..sh {
                for x in 0..sw {
                    let dx = x as f32 - cx;
                    let dy = y as f32 - cy;
                    let d2 = dx * dx + dy * dy;
                    let di = ((y * sw + x) * 4) as usize;
                    if d2 <= ring_inner2 {
                        // Transparent cutout — see the cursor and what's
                        // around it untouched.
                        canvas[di] = 0;
                        canvas[di + 1] = 0;
                        canvas[di + 2] = 0;
                        canvas[di + 3] = 0;
                    } else if d2 <= inner2 {
                        // Bright white ring (premultiplied: RGB == A).
                        canvas[di] = RING_ALPHA;
                        canvas[di + 1] = RING_ALPHA;
                        canvas[di + 2] = RING_ALPHA;
                        canvas[di + 3] = RING_ALPHA;
                    } else if d2 >= outer2 {
                        // Full dim outside the feather.
                        canvas[di] = 0;
                        canvas[di + 1] = 0;
                        canvas[di + 2] = 0;
                        canvas[di + 3] = DIM_ALPHA;
                    } else {
                        // Feather between ring and full dim.
                        let d = d2.sqrt();
                        let t = (d - inner) / FEATHER;
                        let a = (DIM_ALPHA as f32 * t) as u8;
                        canvas[di] = 0;
                        canvas[di + 1] = 0;
                        canvas[di + 2] = 0;
                        canvas[di + 3] = a;
                    }
                }
            }
        }

        out.surface.damage_buffer(0, 0, sw as i32, sh as i32);
        // Only request the next frame callback while active. Inactive
        // surfaces don't need to keep redrawing — they only need a single
        // transparent commit to clear any prior dim.
        if out.active {
            out.surface.frame(qh, out.surface.clone());
        }
        let _ = buf.attach_to(&out.surface);
        out.surface.commit();
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
        // Frame callback: redraw if still active. Inactive surfaces don't
        // request callbacks (see draw_output) so they don't get here.
        if let Some(idx) = self.outputs.iter().position(|o| &o.surface == surface)
            && self.outputs[idx].active
        {
            self.draw_output(idx, qh);
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
        // Collect indices to redraw + dismiss flag, then act, to avoid
        // borrow conflicts (we mutate outputs while iterating events).
        let mut to_redraw: Vec<usize> = Vec::new();
        let mut dismiss = false;

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
                    let pos = event.position;
                    self.outputs[idx].cursor = Some((pos.0 as f32, pos.1 as f32));
                    let was_active = self.outputs[idx].active;
                    self.outputs[idx].active = true;
                    if !was_active {
                        to_redraw.push(idx);
                    }
                    // Any other currently-active surfaces lose focus —
                    // schedule a transparent redraw to clear them.
                    for i in 0..self.outputs.len() {
                        if i != idx && self.outputs[i].active {
                            self.outputs[i].active = false;
                            to_redraw.push(i);
                        }
                    }
                }
                PointerEventKind::Leave { .. } => {
                    if self.outputs[idx].active {
                        self.outputs[idx].active = false;
                        self.outputs[idx].cursor = None;
                        to_redraw.push(idx);
                    }
                }
                PointerEventKind::Press { .. } => {
                    dismiss = true;
                }
                _ => {}
            }
        }

        for idx in to_redraw {
            self.draw_output(idx, qh);
        }
        if dismiss {
            self.exit = true;
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
        // Initial draw: inactive (transparent). Becomes active when the
        // user moves the mouse onto this output.
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

delegate_compositor!(State);
delegate_keyboard!(State);
delegate_layer!(State);
delegate_output!(State);
delegate_pointer!(State);
delegate_registry!(State);
delegate_seat!(State);
delegate_shm!(State);

//! "Find Mouse" tool — fullscreen dim overlay with a circular cutout to
//! draw the eye to the cursor.
//!
//! v0.3.0 first cut: render once on layer-surface configure (cursor
//! position fallback to screen center because COSMIC doesn't fire
//! Pointer.Enter for fresh layer surfaces), hold for `DURATION_MS`,
//! exit. No motion-tracking, no animation, no dismiss-on-move yet.
//! v0.3.x will turn this into a proper event loop with cursor follow,
//! fade in/out, expanding ring effect.

use std::io;
use std::time::Duration;

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_layer, delegate_output, delegate_pointer,
    delegate_registry, delegate_seat, delegate_shm,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
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
    protocol::{wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
};

const DURATION_MS: u64 = 800;
const SPOTLIGHT_RADIUS: f32 = 90.0;
const FEATHER: f32 = 28.0;
const DIM_ALPHA: u8 = 140;

pub fn show() -> io::Result<()> {
    let conn = Connection::connect_to_env().map_err(io::Error::other)?;
    let (globals, mut event_queue) =
        registry_queue_init(&conn).map_err(io::Error::other)?;
    let qh: QueueHandle<State> = event_queue.handle();

    let compositor = CompositorState::bind(&globals, &qh).map_err(io::Error::other)?;
    let layer_shell = LayerShell::bind(&globals, &qh).map_err(io::Error::other)?;
    let shm = Shm::bind(&globals, &qh).map_err(io::Error::other)?;
    let pool = SlotPool::new(16 * 1024 * 1024, &shm).map_err(io::Error::other)?;

    let mut state = State {
        registry_state: RegistryState::new(&globals),
        seat_state: SeatState::new(&globals, &qh),
        output_state: OutputState::new(&globals, &qh),
        compositor,
        layer_shell,
        shm,
        pool,
        outputs: Vec::new(),
        pointer: None,
    };

    // Three roundtrips: first to register outputs + bind seat, second to
    // process configure events that trigger draws, third to flush the
    // buffer commits those draws queued. (Roundtrip flushes BEFORE
    // dispatch, so requests we queue during dispatch sit in the outgoing
    // buffer until the next flush — without RT #3 the compositor never
    // sees our buffers and the overlay is invisible.)
    event_queue
        .roundtrip(&mut state)
        .map_err(io::Error::other)?;
    event_queue
        .roundtrip(&mut state)
        .map_err(io::Error::other)?;
    event_queue
        .roundtrip(&mut state)
        .map_err(io::Error::other)?;

    // Hold the spotlight on screen. The layer surfaces tear down via Drop
    // on `state` once we return.
    std::thread::sleep(Duration::from_millis(DURATION_MS));

    Ok(())
}

struct OutputSurface {
    wl_output: wl_output::WlOutput,
    layer: LayerSurface,
    surface: wl_surface::WlSurface,
    size: (u32, u32),
    configured: bool,
    /// Surface-local cursor position. None means COSMIC hasn't fired a
    /// Pointer.Enter for our surface yet — we draw with a center fallback.
    cursor: Option<(f32, f32)>,
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
    pointer: Option<wl_pointer::WlPointer>,
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
        // Don't grab keyboard — find_mouse should not steal focus.
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer.commit();

        self.outputs.push(OutputSurface {
            wl_output,
            layer,
            surface,
            size,
            configured: false,
            cursor: None,
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

        // Cursor with screen-center fallback.
        let (cx, cy) = out
            .cursor
            .unwrap_or((sw as f32 * 0.5, sh as f32 * 0.5));
        let inner = SPOTLIGHT_RADIUS;
        let outer = SPOTLIGHT_RADIUS + FEATHER;
        let inner2 = inner * inner;
        let outer2 = outer * outer;

        // Fill with black + variable alpha. Buffer is premultiplied
        // ARGB8888 (BGRA byte order on little-endian); since base color is
        // black, premultiplied channels stay 0 and only alpha varies.
        for y in 0..sh {
            for x in 0..sw {
                let dx = x as f32 - cx;
                let dy = y as f32 - cy;
                let d2 = dx * dx + dy * dy;
                let alpha = if d2 <= inner2 {
                    0u8
                } else if d2 >= outer2 {
                    DIM_ALPHA
                } else {
                    let d = d2.sqrt();
                    let t = (d - inner) / FEATHER;
                    (DIM_ALPHA as f32 * t) as u8
                };
                let di = ((y * sw + x) * 4) as usize;
                canvas[di] = 0;
                canvas[di + 1] = 0;
                canvas[di + 2] = 0;
                canvas[di + 3] = alpha;
            }
        }

        out.surface.damage_buffer(0, 0, sw as i32, sh as i32);
        let _ = buf.attach_to(&out.surface);
        out.surface.commit();
        let _ = qh; // qh unused in single-frame path
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
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: u32,
    ) {
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
        if capability == Capability::Pointer && self.pointer.is_none() {
            self.pointer = self.seat_state.get_pointer(qh, &seat).ok();
        }
    }
    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer
            && let Some(p) = self.pointer.take()
        {
            p.release();
        }
    }
    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl PointerHandler for State {
    fn pointer_frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        // Best-effort cursor capture for outputs whose surfaces happen to
        // see the pointer before our two roundtrips finish. The single-
        // frame draw path doesn't update on subsequent motion.
        for event in events {
            let Some(idx) = self
                .outputs
                .iter()
                .position(|o| o.surface == event.surface)
            else {
                continue;
            };
            if matches!(
                event.kind,
                PointerEventKind::Enter { .. } | PointerEventKind::Motion { .. }
            ) {
                let (x, y) = event.position;
                self.outputs[idx].cursor = Some((x as f32, y as f32));
            }
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
delegate_layer!(State);
delegate_output!(State);
delegate_pointer!(State);
delegate_registry!(State);
delegate_seat!(State);
delegate_shm!(State);

//! rt — a fast, tiling terminal emulator; a loose Rust port of Terminator.
//!
//! This file is the GL front-end and winit run-loop. It cannot be unit-tested
//! (it needs a display and a GPU), so the testable pieces live elsewhere:
//! layout in `rt-core`, the engine in `rt-engine`, the controller in
//! `rt-session`, keybindings in `rt-config`, and key translation in
//! `rt_app::input`. Here we only *wire* those together and drive the GPU.
//!
//! Flow per key press: winit event → `chord_from_winit` → keymap lookup. A hit
//! becomes an `Action` fed to `Session::apply`; a miss becomes PTY bytes via
//! `encode_key` fed to `Session::feed_input` (respecting broadcast mode).

mod input; // (also re-exported by lib.rs for tests; declared here for the bin)
mod render; // the GL glyph-atlas renderer

use std::num::NonZeroU32; // required by glutin's surface resize API
use std::time::{Duration, Instant}; // frame pacing for async PTY updates

use glutin::config::ConfigTemplateBuilder;
use glutin::context::ContextAttributesBuilder;
use glutin::display::GetGlDisplay;
use glutin::prelude::*; // brings the Gl* traits (make_current, get_proc_address, …)
use glutin::surface::{Surface, SurfaceAttributesBuilder, WindowSurface};
use glutin_winit::{DisplayBuilder, GlWindow};
use raw_window_handle::HasWindowHandle;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::ModifiersState;
use winit::window::{Window, WindowId};

use render::{Color, Renderer};
use rt_config::Keymap;
use rt_core::Rect;
use rt_engine::TermPane;
use rt_session::{Session, SessionEvent};

/// The concrete session type used by the app: real PTY panes, spawned by a
/// boxed closure (boxed so the `Session`'s factory type is nameable in a field).
type AppSession = Session<TermPane, Box<dyn FnMut(usize, usize) -> TermPane>>;

/// Everything that only exists once a window and GL context are created (which
/// happens on the first `resumed`). Kept in an `Option` on the `App` so we can
/// build it lazily and tear it down on suspend.
struct Active {
    window: Window,                       // the OS window
    surface: Surface<WindowSurface>,      // the GL drawing surface
    context: glutin::context::PossiblyCurrentContext, // the current GL context
    renderer: Renderer,                   // our glyph-atlas renderer
    session: AppSession,                  // layout + panes + focus + broadcast
    keymap: Keymap,                       // Terminator-style bindings
    mods: ModifiersState,                 // live modifier state (updated on change)
}

/// The winit application object. Holds only the font bytes until `resumed`
/// builds the `Active` state.
struct App {
    font: Vec<u8>,        // TrueType bytes for the monospace font
    active: Option<Active>, // populated on first resume
}

/// Locate a monospace TrueType font on the system. rt does not ship one (to
/// avoid bundling a font binary in git); it probes the usual Linux locations.
/// Returns the font bytes, or `None` if none is found (the app then exits with
/// a helpful message).
fn load_font() -> Option<Vec<u8>> {
    // Common monospace fonts present on virtually all Linux desktops, in order
    // of preference.
    const CANDIDATES: &[&str] = &[
        "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
        "/usr/share/fonts/dejavu/DejaVuSansMono.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf",
        "/usr/share/fonts/liberation/LiberationMono-Regular.ttf",
        "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
        "/usr/share/fonts/noto/NotoSansMono-Regular.ttf",
    ];
    for path in CANDIDATES {
        // Try to read each candidate; the first that succeeds wins.
        if let Ok(bytes) = std::fs::read(path) {
            log::info!("using font {path}"); // record which font we picked
            return Some(bytes);
        }
    }
    None // nothing found
}

impl ApplicationHandler for App {
    /// Called when the app is (re)activated. On the first call we build the
    /// window, GL context, renderer, and session. Subsequent calls (after a
    /// suspend) are no-ops here because we keep the state alive.
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.active.is_some() {
            return; // already initialised; nothing to do on re-resume
        }

        // --- create the window and choose a GL config --------------------
        let window_attrs = Window::default_attributes()
            .with_title("rt") // window title; per-pane titles update it later
            .with_inner_size(winit::dpi::LogicalSize::new(960.0, 600.0)); // sensible default
        let template = ConfigTemplateBuilder::new().with_alpha_size(8); // want an alpha channel
        // DisplayBuilder creates the window AND enumerates GL configs together,
        // which is the supported winit-0.30/glutin-0.32 pattern.
        let display_builder = DisplayBuilder::new().with_window_attributes(Some(window_attrs));
        let (window, gl_config) = match display_builder.build(event_loop, template, |configs| {
            // Pick the config with the most samples (nicest); fall back to first.
            configs
                .reduce(|a, b| if b.num_samples() > a.num_samples() { b } else { a })
                .expect("at least one GL config")
        }) {
            Ok((Some(window), config)) => (window, config), // got a window + config
            Ok((None, _)) => {
                log::error!("window creation returned no window");
                event_loop.exit();
                return;
            }
            Err(e) => {
                log::error!("failed to create window/GL config: {e}");
                event_loop.exit();
                return;
            }
        };

        // --- create the GL context and surface ---------------------------
        let gl_display = gl_config.display(); // the platform GL display
        // Raw handle needed to bind the context to this specific window.
        let raw_handle = window.window_handle().ok().map(|h| h.as_raw());
        let context_attrs = ContextAttributesBuilder::new().build(raw_handle);
        // Create a not-yet-current context, then a surface, then make current.
        let not_current = match unsafe { gl_display.create_context(&gl_config, &context_attrs) } {
            Ok(c) => c,
            Err(e) => {
                log::error!("GL context creation failed: {e}");
                event_loop.exit();
                return;
            }
        };
        // Build the window surface at the window's current size.
        let attrs = window.build_surface_attributes(SurfaceAttributesBuilder::new());
        let attrs = match attrs {
            Ok(a) => a,
            Err(e) => {
                log::error!("surface attributes failed: {e}");
                event_loop.exit();
                return;
            }
        };
        let surface = match unsafe { gl_display.create_window_surface(&gl_config, &attrs) } {
            Ok(s) => s,
            Err(e) => {
                log::error!("GL surface creation failed: {e}");
                event_loop.exit();
                return;
            }
        };
        // Make the context current on the surface so GL calls target it.
        let context = match not_current.make_current(&surface) {
            Ok(c) => c,
            Err(e) => {
                log::error!("make_current failed: {e}");
                event_loop.exit();
                return;
            }
        };

        // --- load GL function pointers into glow -------------------------
        let gl = unsafe {
            glow::Context::from_loader_function_cstr(|s| gl_display.get_proc_address(s).cast())
        };

        // --- build the renderer ------------------------------------------
        let mut renderer = match Renderer::new(gl, &self.font, 18.0) {
            Ok(r) => r,
            Err(e) => {
                log::error!("renderer init failed: {e}");
                event_loop.exit();
                return;
            }
        };
        // Size the renderer/viewport to the window's physical pixels.
        let size = window.inner_size(); // physical pixel size
        renderer.resize(size.width as f32, size.height as f32);
        let cell = renderer.cell_size(); // (cell_w, cell_h) in pixels

        // --- build the session with real PTY panes -----------------------
        let bounds = Rect::new(0.0, 0.0, size.width as f32, size.height as f32);
        // The factory spawns a shell-backed pane at the requested cell size. A
        // spawn failure here is fatal (no PTY available) — we surface it by
        // panicking with a clear message rather than rendering a broken pane.
        let spawn: Box<dyn FnMut(usize, usize) -> TermPane> = Box::new(|cols, rows| {
            TermPane::spawn(None, None, cols.max(1), rows.max(1))
                .expect("failed to spawn a PTY/shell for a pane")
        });
        let session = Session::new(bounds, cell, spawn);

        // Store the fully-initialised state and paint once.
        self.active = Some(Active {
            window,
            surface,
            context,
            renderer,
            session,
            keymap: Keymap::defaults(),
            mods: ModifiersState::empty(),
        });
        // Poll so we keep re-checking PTYs for async output even without input.
        event_loop.set_control_flow(ControlFlow::Poll);
        if let Some(active) = &self.active {
            active.window.request_redraw(); // first paint
        }
    }

    /// Handle a window event: close, resize, key input, redraw.
    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        // Everything here needs the active state; ignore events before resume.
        let Some(active) = self.active.as_mut() else { return };

        match event {
            // The user closed the window.
            WindowEvent::CloseRequested => event_loop.exit(),

            // Track modifier state so key events can build correct chords.
            WindowEvent::ModifiersChanged(new_mods) => {
                active.mods = new_mods.state(); // remember Ctrl/Shift/Alt/Super
            }

            // Window resized: resize the GL surface, viewport, and relayout PTYs.
            WindowEvent::Resized(size) => {
                // glutin needs non-zero dimensions to resize the surface.
                if let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) {
                    active.surface.resize(&active.context, w, h); // resize GL surface
                }
                active.renderer.resize(size.width as f32, size.height as f32); // viewport
                // Recompute pane sizes from the new window bounds.
                let bounds = Rect::new(0.0, 0.0, size.width as f32, size.height as f32);
                active.session.relayout(bounds); // push new sizes to PTYs
                active.window.request_redraw(); // repaint at the new size
            }

            // A key was pressed or released.
            WindowEvent::KeyboardInput { event: key_event, .. } => {
                // Only act on presses (ignore auto-repeat=false-positives and releases).
                if key_event.state != ElementState::Pressed {
                    return;
                }
                self.on_key_press(key_event);
            }

            // Time to paint.
            WindowEvent::RedrawRequested => {
                self.redraw();
            }

            _ => {} // ignore the many other window events for now
        }
    }

    /// Called whenever the loop is about to block. We use it to poll each pane
    /// for asynchronous PTY output and request a redraw when anything changed,
    /// so terminal output appears without the user touching the keyboard.
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(active) = self.active.as_mut() else { return };
        // Drain events from every live pane; if any produced output/title/bell,
        // we need to repaint.
        let mut dirty = false; // did anything change this tick?
        for id in active.session.tree().all_panes() {
            if let Some(pane) = active.session.pane(id) {
                // Any drained event means the grid or title may have changed.
                if !pane.drain_events().is_empty() {
                    dirty = true;
                }
            }
        }
        if dirty {
            active.window.request_redraw(); // schedule a paint
        }
        // Re-check about every 16ms (~60fps) even when idle, so async output is
        // picked up promptly without a hot busy-loop.
        event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(16)));
    }
}

impl App {
    /// Handle one key *press*: translate to a chord, look it up, and either run
    /// the bound action or type the key into the focused PTY(s).
    fn on_key_press(&mut self, key_event: winit::event::KeyEvent) {
        let Some(active) = self.active.as_mut() else { return };
        let mods = active.mods; // current modifier state
        // First, is this chord bound to an rt action?
        if let Some(chord) = input::chord_from_winit(&key_event.logical_key, mods) {
            if let Some(action) = active.keymap.action_for(&chord) {
                // Run the action; handle any session event it returns.
                if let Some(ev) = active.session.apply(action) {
                    match ev {
                        SessionEvent::CloseWindow => {
                            // Last pane closed / close_window pressed: exit.
                            // (We can't reach the ActiveEventLoop here; set a
                            // flag by requesting a redraw that will observe an
                            // empty tree — simplest is to exit via the window.)
                            // For now, drop active state to close the app.
                            self.active = None;
                            std::process::exit(0); // clean exit; PTYs shut down on drop
                        }
                        SessionEvent::Redraw => active.window.request_redraw(),
                        // Clipboard is not yet wired to the OS; ignore for now.
                        SessionEvent::Copy | SessionEvent::Paste => {}
                    }
                }
                return; // the key was consumed as an action
            }
        }
        // Not a binding: treat as ordinary typing. Encode to PTY bytes and feed
        // the focused pane (or the broadcast set).
        if let Some(bytes) = input::encode_key(&key_event.logical_key, mods) {
            active.session.feed_input(&bytes); // send to the shell(s)
        }
    }

    /// Repaint the whole window: fill each pane's background, draw its visible
    /// grid, then outline the focused pane. Finally swap buffers.
    fn redraw(&mut self) {
        let Some(active) = self.active.as_mut() else { return };
        // Terminal colours (a dark theme): near-black bg, light-grey fg.
        let bg = Color::rgb(0x10, 0x10, 0x14); // window/pane background
        let fg = Color::rgb(0xd0, 0xd0, 0xd8); // default text colour
        let focus_border = Color::rgb(0x4a, 0x90, 0xd9); // blue focus outline

        let size = active.window.inner_size(); // physical pixels
        let bounds = Rect::new(0.0, 0.0, size.width as f32, size.height as f32);
        active.renderer.begin_frame(bg); // clear to bg

        let focus = active.session.focus(); // which pane is focused
        // Draw every visible pane.
        for (id, rect) in active.session.tree().rects(bounds) {
            // Fill this pane's background (in case the clear colour differs).
            active.renderer.fill_rect(rect.x, rect.y, rect.w, rect.h, bg);
            // Copy the pane's current grid and draw each non-blank cell.
            if let Some(pane) = active.session.pane(id) {
                let snap = pane.snapshot(); // immutable grid copy
                for (row_idx, row) in snap.rows.iter().enumerate() {
                    for (col_idx, cell) in row.iter().enumerate() {
                        // Skip spaces to save quads (bg already drawn).
                        if cell.c != ' ' {
                            active.renderer.draw_char(rect.x, rect.y, col_idx, row_idx, cell.c, fg);
                        }
                    }
                }
            }
            // Outline the focused pane with a thin border (four thin rects).
            if id == focus {
                let t = 2.0; // border thickness in pixels
                active.renderer.fill_rect(rect.x, rect.y, rect.w, t, focus_border); // top
                active.renderer.fill_rect(rect.x, rect.bottom() - t, rect.w, t, focus_border); // bottom
                active.renderer.fill_rect(rect.x, rect.y, t, rect.h, focus_border); // left
                active.renderer.fill_rect(rect.right() - t, rect.y, t, rect.h, focus_border); // right
            }
        }

        active.renderer.end_frame(); // upload + draw call
        // Present the frame.
        if let Err(e) = active.surface.swap_buffers(&active.context) {
            log::error!("swap_buffers failed: {e}"); // non-fatal; log and continue
        }
    }
}

/// Program entry point: set up logging, load a font, and run the winit loop.
fn main() {
    env_logger::init(); // honour RUST_LOG for diagnostics
    // A font is mandatory; fail early with guidance if none is installed.
    let font = match load_font() {
        Some(f) => f,
        None => {
            eprintln!(
                "rt: no monospace font found. Install e.g. 'fonts-dejavu-core' \
                 (Debian/Ubuntu) or 'ttf-dejavu' (Arch) and retry."
            );
            std::process::exit(1);
        }
    };
    // Build the winit event loop and hand it our application.
    let event_loop = EventLoop::new().expect("failed to create event loop");
    let mut app = App { font, active: None };
    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("rt: event loop error: {e}"); // surface any run-loop failure
        std::process::exit(1);
    }
}

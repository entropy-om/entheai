use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader};
use std::num::NonZeroU32;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use clap::Parser;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{Event, MouseButton, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoop};
use winit::window::{Window, WindowLevel};

use entheai_companion::qr::{self, SessionPayload};
use entheai_companion::render::{self, AnimationState};
use entheai_companion::state::StateChange;

/// The softbuffer context/surface pair, created once and reused across
/// frames instead of being rebuilt on every `RedrawRequested`.
type SoftbufferState = (
    softbuffer::Context<Rc<Window>>,
    softbuffer::Surface<Rc<Window>, Rc<Window>>,
);

/// entheai companion — a tiny session beacon window.
#[derive(Parser)]
struct Cli {
    /// Session UUID.
    #[arg(long)]
    session_id: String,

    /// Tailscale or local hostname.
    #[arg(long, default_value = "localhost")]
    host: String,

    /// Port for remote session endpoint.
    #[arg(long, default_value = "9876")]
    port: u16,

    /// Working directory.
    #[arg(long)]
    cwd: Option<String>,

    /// Disable always-on-top.
    #[arg(long)]
    no_always_on_top: bool,

    /// Path to the Unix domain socket for state events.
    #[arg(long)]
    socket: Option<PathBuf>,

    /// Target render frame rate.
    #[arg(long, default_value_t = render::FPS)]
    fps: f64,
}

const ADJECTIVES: [&str; 16] = [
    "quiet", "brisk", "amber", "cobalt", "drift", "ember", "frost", "gilded", "hollow", "inky",
    "jagged", "keen", "lush", "murky", "nimble", "opal",
];

const NOUNS: [&str; 16] = [
    "lynx", "otter", "raven", "heron", "badger", "falcon", "marten", "wren", "vole", "gecko",
    "ibex", "civet", "puffin", "serval", "tapir", "wombat",
];

/// Derives a stable, human-friendly codename (`"{adjective}-{noun}"`) from a
/// session id. Uses `DefaultHasher`, which hashes with fixed keys, so the
/// same session id always maps to the same codename — no RNG, no new crates.
fn codename(session_id: &str) -> String {
    let mut hasher = DefaultHasher::new();
    session_id.hash(&mut hasher);
    let hash = hasher.finish();
    let adj = ADJECTIVES[(hash as usize) % ADJECTIVES.len()];
    let noun = NOUNS[((hash >> 32) as usize) % NOUNS.len()];
    format!("{adj}-{noun}")
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let name = codename(&cli.session_id);
    eprintln!("companion '{name}' (session {})", cli.session_id);
    let fps = cli.fps;

    let cwd = cli.cwd.unwrap_or_else(|| {
        std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default()
    });

    let payload = SessionPayload {
        v: 1,
        sid: cli.session_id.clone(),
        host: cli.host.clone(),
        port: cli.port,
        cwd,
        name: name.clone(),
    };

    let qr_grid = qr::generate(&payload)?;
    let session_url = format!(
        "http://{}.local:{}/session/{}",
        cli.host, cli.port, payload.sid
    );

    // Connect to the Unix socket (non-blocking) if provided.
    let socket_reader = cli.socket.and_then(|path| {
        let stream = UnixStream::connect(&path).ok()?;
        stream.set_nonblocking(true).ok()?;
        Some(BufReader::new(stream))
    });

    #[allow(deprecated)]
    let event_loop = EventLoop::new()?;

    let window_level = if cli.no_always_on_top {
        WindowLevel::Normal
    } else {
        WindowLevel::AlwaysOnTop
    };

    let window_attrs = Window::default_attributes()
        .with_title(format!("entheai · {name}"))
        .with_decorations(false)
        .with_resizable(false)
        .with_inner_size(LogicalSize::new(180.0, 180.0))
        .with_transparent(true)
        .with_window_level(window_level);

    #[allow(deprecated)]
    let window = Rc::new(event_loop.create_window(window_attrs)?);

    if let Some(monitor) = window.current_monitor() {
        let screen = monitor.size();
        let win_size = window.outer_size();
        let scale = monitor.scale_factor();
        let margin = (20.0 * scale) as i32;
        let x = screen
            .width
            .saturating_sub(win_size.width)
            .saturating_sub(margin as u32) as i32;
        let y = screen
            .height
            .saturating_sub(win_size.height)
            .saturating_sub(margin as u32) as i32;
        window.set_outer_position(PhysicalPosition::new(x.max(0), y.max(0)));
    }

    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::WindowExtMacOS;
        window.set_has_shadow(false);
    }

    let start = Instant::now();
    let mut anim = AnimationState::default();
    let mut socket_reader = socket_reader;
    let mut last_frame = Instant::now();
    let mut fading_since: Option<f64> = None;
    let mut line_buf = String::new();
    let mut surface_state: Option<SoftbufferState> = None;
    let mut shift_held = false;
    // Time of the last non-shift left release, for double-click-to-copy: a lone
    // click on the always-on-top beacon must never clobber the user's clipboard.
    let mut last_click: Option<f64> = None;

    #[allow(deprecated)]
    event_loop.run(move |event, target| {
        let now = start.elapsed().as_secs_f64();

        match event {
            Event::WindowEvent {
                event: WindowEvent::RedrawRequested,
                ..
            } => {
                let size = window.inner_size();
                let (w, h) = (size.width, size.height);
                if w == 0 || h == 0 {
                    return;
                }

                let dt = last_frame.elapsed().as_secs_f64();
                last_frame = Instant::now();
                anim.tick(dt);

                if let Some(fade_start) = fading_since {
                    let elapsed = now - fade_start;
                    anim.fade_alpha = (1.0 - (elapsed / 0.5) as f32).max(0.0);
                    if anim.fade_alpha <= 0.0 {
                        target.exit();
                        return;
                    }
                }

                // Create the softbuffer context/surface once and reuse them for
                // every subsequent frame; only resize (cheap) per frame.
                if surface_state.is_none() {
                    match softbuffer::Context::new(Rc::clone(&window)) {
                        Ok(ctx) => match softbuffer::Surface::new(&ctx, Rc::clone(&window)) {
                            Ok(surf) => surface_state = Some((ctx, surf)),
                            Err(e) => {
                                eprintln!("companion: failed to create softbuffer surface: {e}");
                                return;
                            }
                        },
                        Err(e) => {
                            eprintln!("companion: failed to create softbuffer context: {e}");
                            return;
                        }
                    }
                }

                let Some((_ctx, surf)) = surface_state.as_mut() else {
                    return;
                };

                let _ = surf.resize(NonZeroU32::new(w).unwrap(), NonZeroU32::new(h).unwrap());

                if let Ok(mut buffer) = surf.buffer_mut() {
                    render::render_frame(&mut buffer, w, h, &qr_grid, &anim, now, &name);
                    let _ = buffer.present();
                }
            }

            Event::WindowEvent {
                event: WindowEvent::ModifiersChanged(mods),
                ..
            } => {
                shift_held = mods.state().shift_key();
            }

            // Shift + press -> start an OS-level window drag instead of the
            // normal click-to-copy interaction.
            Event::WindowEvent {
                event:
                    WindowEvent::MouseInput {
                        state: winit::event::ElementState::Pressed,
                        button: MouseButton::Left,
                        ..
                    },
                ..
            } => {
                if shift_held {
                    let _ = window.drag_window();
                }
            }

            // Click anywhere (without Shift) -> copy session URL to clipboard.
            Event::WindowEvent {
                event:
                    WindowEvent::MouseInput {
                        state: winit::event::ElementState::Released,
                        button: MouseButton::Left,
                        ..
                    },
                ..
            } => {
                // Only a deliberate DOUBLE-click copies the session URL; a lone
                // click does nothing, so an accidental click never overwrites
                // whatever the user currently has on their clipboard.
                if !shift_held {
                    let is_double = last_click.is_some_and(|t| now - t < 0.4);
                    if is_double {
                        anim.flash(now);
                        if let Ok(mut clipboard) = arboard::Clipboard::new() {
                            let _ = clipboard.set_text(&session_url);
                        }
                        last_click = None;
                    } else {
                        last_click = Some(now);
                    }
                }
            }

            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                target.exit();
            }

            Event::AboutToWait => {
                // Drain any pending socket events. `line_buf` persists across
                // `AboutToWait` calls so a `StateChange` line split across two
                // non-blocking reads (WouldBlock mid-line) isn't discarded —
                // it's only cleared once a full line has been parsed.
                if let Some(ref mut reader) = socket_reader {
                    loop {
                        match reader.read_line(&mut line_buf) {
                            Ok(0) => {
                                socket_reader = None;
                                fading_since = Some(now);
                                break;
                            }
                            Ok(_) => {
                                if line_buf.ends_with('\n') {
                                    let trimmed = line_buf.trim();
                                    if !trimmed.is_empty() {
                                        if let Ok(change) =
                                            serde_json::from_str::<StateChange>(trimmed)
                                        {
                                            anim.set_state(change.state);
                                        }
                                    }
                                    line_buf.clear();
                                }
                                // else: partial line without a trailing newline
                                // yet; keep buffering on the next drain.
                            }
                            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                                break;
                            }
                            Err(_) => {
                                socket_reader = None;
                                fading_since = Some(now);
                                break;
                            }
                        }
                    }
                }

                // Cap redraws to the render module's frame budget instead of
                // polling flat-out; sleep until the next frame is due.
                let interval = render::frame_interval(fps);
                if last_frame.elapsed() >= interval {
                    window.request_redraw();
                    last_frame = Instant::now();
                }
                target.set_control_flow(ControlFlow::WaitUntil(last_frame + interval));
            }

            _ => {}
        }
    })?;

    Ok(())
}

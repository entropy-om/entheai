use std::io::{BufRead, BufReader};
use std::num::NonZeroU32;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Instant;

use clap::Parser;
use softbuffer::Surface;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{Event, MouseButton, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoop};
use winit::window::{Window, WindowLevel};

use entheai_companion::qr::{self, SessionPayload};
use entheai_companion::render::{self, AnimationState};
use entheai_companion::state::StateChange;

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
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

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
        .with_title("entheai companion")
        .with_decorations(false)
        .with_resizable(false)
        .with_inner_size(LogicalSize::new(180.0, 180.0))
        .with_transparent(true)
        .with_window_level(window_level);

    #[allow(deprecated)]
    let window = event_loop.create_window(window_attrs)?;

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

    #[allow(deprecated)]
    event_loop.run(move |event, target| {
        target.set_control_flow(ControlFlow::Poll);

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

                let now = start.elapsed().as_secs_f64();
                let dt = last_frame.elapsed().as_secs_f64();
                last_frame = Instant::now();
                anim.tick(dt);

                let ctx = softbuffer::Context::new(&window).expect("softbuffer context");
                let mut surf = Surface::new(&ctx, &window).expect("softbuffer surface");

                let _ = surf.resize(NonZeroU32::new(w).unwrap(), NonZeroU32::new(h).unwrap());

                if let Ok(mut buffer) = surf.buffer_mut() {
                    render::render_frame(&mut buffer, w, h, &qr_grid, &anim, now);
                    let _ = buffer.present();
                }
            }

            // Click anywhere -> copy session URL to clipboard.
            Event::WindowEvent {
                event:
                    WindowEvent::MouseInput {
                        state: winit::event::ElementState::Released,
                        button: MouseButton::Left,
                        ..
                    },
                ..
            } => {
                let now = start.elapsed().as_secs_f64();
                anim.flash(now);
                if let Ok(mut clipboard) = arboard::Clipboard::new() {
                    let _ = clipboard.set_text(&session_url);
                }
            }

            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                target.exit();
            }

            Event::AboutToWait => {
                // Drain any pending socket events.
                if let Some(ref mut reader) = socket_reader {
                    loop {
                        let mut line = String::new();
                        match reader.read_line(&mut line) {
                            Ok(0) => {
                                socket_reader = None;
                                break;
                            }
                            Ok(_) => {
                                let trimmed = line.trim();
                                if !trimmed.is_empty() {
                                    if let Ok(change) = serde_json::from_str::<StateChange>(trimmed)
                                    {
                                        anim.set_state(change.state);
                                    }
                                }
                            }
                            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                                break;
                            }
                            Err(_) => {
                                socket_reader = None;
                                break;
                            }
                        }
                    }
                }

                window.request_redraw();
            }

            _ => {}
        }
    })?;

    Ok(())
}

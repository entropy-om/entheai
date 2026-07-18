use std::num::NonZeroU32;
use std::time::Instant;

use clap::Parser;
use softbuffer::Surface;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{Event, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoop};
use winit::window::{Window, WindowLevel};

use entheai_companion::qr::{self, SessionPayload};
use entheai_companion::render;

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
        sid: cli.session_id,
        host: cli.host,
        port: cli.port,
        cwd,
    };

    let qr_grid = qr::generate(&payload)?;

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

    // Position at bottom-right with 20px margin.
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

                let ctx = softbuffer::Context::new(&window).expect("softbuffer context");
                let mut surf = Surface::new(&ctx, &window).expect("softbuffer surface");

                let _ = surf.resize(NonZeroU32::new(w).unwrap(), NonZeroU32::new(h).unwrap());

                if let Ok(mut buffer) = surf.buffer_mut() {
                    let elapsed = start.elapsed().as_secs_f64();
                    render::render_frame(&mut buffer, w, h, &qr_grid, elapsed);
                    let _ = buffer.present();
                }
            }

            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                target.exit();
            }

            Event::AboutToWait => {
                window.request_redraw();
            }

            _ => {}
        }
    })?;

    Ok(())
}

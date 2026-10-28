use std::{io::Read, sync::Arc};

use clap::Parser;
use gpapi::{
  clap::InfoLevelVerbosity,
  service::transport::{MAX_CREDENTIAL_FRAME, SessionCredential},
};
use log::info;

use crate::{app::App, settings::LaunchSettings};

const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), " (", compile_time::date_str!(), ")");

#[derive(Parser)]
#[command(name = "nusaprotect", version = VERSION)]
struct Cli {
  #[arg(long, help = "Read the service credential from stdin")]
  service_credential_on_stdin: bool,

  #[arg(long, help = "Start the window minimized")]
  minimized: bool,

  #[command(flatten)]
  verbose: InfoLevelVerbosity,
}

impl Cli {
  fn run(&self, settings: &LaunchSettings) -> anyhow::Result<()> {
    let credential = self.read_credential()?;
    let app = App::new(
      Arc::new(credential),
      self.minimized || settings.start_minimized,
      settings.symbolic_tray_icon,
    );

    app.run()
  }

  fn read_credential(&self) -> anyhow::Result<SessionCredential> {
    if !self.service_credential_on_stdin {
      anyhow::bail!("Service credential must be provided via --service-credential-on-stdin");
    }

    let stdin = std::io::stdin();
    let mut input = stdin.lock();

    let mut header = [0_u8; 2];
    input.read_exact(&mut header)?;
    let payload_len = u16::from_be_bytes(header) as usize;
    if payload_len == 0 || payload_len + header.len() > MAX_CREDENTIAL_FRAME {
      anyhow::bail!("Invalid service credential length");
    }

    let mut frame = Vec::with_capacity(header.len() + payload_len);
    frame.extend_from_slice(&header);
    frame.resize(header.len() + payload_len, 0);
    input.read_exact(&mut frame[header.len()..])?;

    SessionCredential::decode_frame(&frame).map_err(|e| anyhow::anyhow!("Invalid service credential: {}", e))
  }
}

fn is_kde_session() -> bool {
  std::env::var("XDG_CURRENT_DESKTOP")
    .map(|d| d.to_ascii_lowercase().contains("kde"))
    .unwrap_or(false)
    || std::env::var("KDE_FULL_SESSION").is_ok()
    || std::env::var("KDE_SESSION_VERSION").is_ok()
}

fn configure_display_backend(hidpi: bool) -> &'static str {
  // Respect an explicitly set GDK_BACKEND
  if std::env::var("GDK_BACKEND").is_ok() {
    return "explicit GDK_BACKEND";
  }

  let is_wayland = std::env::var("WAYLAND_DISPLAY").is_ok()
    || std::env::var("XDG_SESSION_TYPE")
      .map(|t| t.eq_ignore_ascii_case("wayland"))
      .unwrap_or(false);

  unsafe {
    let label = if is_wayland && !is_kde_session() {
      std::env::set_var("GDK_BACKEND", "wayland");
      "Wayland"
    } else {
      std::env::set_var("GDK_BACKEND", "x11");
      std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1");
      if is_wayland {
        "XWayland (KDE session)"
      } else {
        "X11"
      }
    };

    // Classic GTK3/WebKitGTK 4K compatibility: integer-scale the UI when the
    // user enabled HiDPI and hasn't configured GDK scaling themselves.
    if hidpi && std::env::var("GDK_SCALE").is_err() {
      std::env::set_var("GDK_SCALE", "2");
      std::env::set_var("GDK_DPI_SCALE", "1");
      match label {
        "Wayland" => "Wayland (HiDPI)",
        "X11" => "X11 (HiDPI)",
        _ => "XWayland (KDE session, HiDPI)",
      }
    } else {
      label
    }
  }
}

fn init_logger(cli: &Cli) {
  env_logger::builder()
    .filter_level(cli.verbose.log_level_filter())
    .init();
}

pub fn run() {
  let cli = Cli::parse();

  // Load launch settings and configure the display backend before any other
  // threads exist: std::env::set_var is unsafe once the tokio runtime (and the
  // webview) has spawned threads.
  let launch_settings = LaunchSettings::load();
  let backend = configure_display_backend(launch_settings.hidpi);

  init_logger(&cli);
  info!("nusaprotect started: {}", VERSION);
  info!("Display backend: {}", backend);

  // Ambient multi-threaded runtime for cli.run() and the tasks it spawns.
  let runtime = tokio::runtime::Builder::new_multi_thread()
    .enable_all()
    .build()
    .expect("Failed to build the tokio runtime");

  if let Err(e) = runtime.block_on(async { cli.run(&launch_settings) }) {
    eprintln!("{}", e);
    std::process::exit(1);
  }
}

use std::{sync::Arc, time::Duration};

use common::constants::GP_USER_AGENT;
use gpapi::{
  hip::HipSource,
  os_profile::{ClientOs, default_os_version},
  service::{request::WsRequest, vpn_state::VpnState},
};
use log::{info, warn};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};
use tokio::sync::{broadcast, Mutex, oneshot};

use crate::{
  connect::{self, CachedAuth, CertConfig, GatewayPick, OsProfileConfig, PreloginInfo, TunnelConfig},
  service_client::ServiceClient,
  settings::LaunchSettings,
};

pub struct AppState {
  /// Arc so it can be cloned into 'static closures without lifetime issues
  pub client: Arc<Mutex<Option<ServiceClient>>>,
  pub auth_executable: Arc<Mutex<Option<String>>>,
  /// When true, the next SAML auth will pass --clean to clear stored cookies
  pub clean_next_auth: Arc<Mutex<bool>>,
  /// Sender for an in-flight gateway selection prompt
  pub pending_gateway: Arc<Mutex<Option<oneshot::Sender<String>>>>,
  /// Cancellation token for an in-flight connect (SAML auth, gateway login)
  pub connect_cancel: Arc<Mutex<Option<tokio_util::sync::CancellationToken>>>,
  /// Last successful auth artifacts, reused for gateway switching
  pub cached_auth: Arc<Mutex<Option<CachedAuth>>>,
  /// Broadcasts every VpnState received from gpservice to internal consumers
  /// (e.g. switch_gateway waits for Disconnected before reconnecting)
  pub vpn_state_tx: broadcast::Sender<VpnState>,
}

impl AppState {
  pub fn new() -> Self {
    Self {
      client: Arc::new(Mutex::new(None)),
      auth_executable: Arc::new(Mutex::new(None)),
      clean_next_auth: Arc::new(Mutex::new(false)),
      pending_gateway: Arc::new(Mutex::new(None)),
      connect_cancel: Arc::new(Mutex::new(None)),
      cached_auth: Arc::new(Mutex::new(None)),
      vpn_state_tx: broadcast::channel(16).0,
    }
  }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OsDefaults {
  pub os_version: String,
  pub client_version: String,
  pub user_agent: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionDefaults {
  pub linux: OsDefaults,
  pub windows: OsDefaults,
  pub macos: OsDefaults,
}

fn make_os_defaults(client_os: &ClientOs) -> OsDefaults {
  let os_version = default_os_version(client_os);
  let client_version = client_os.default_client_version();
  OsDefaults {
    os_version,
    client_version: client_version.to_owned(),
    user_agent: format!("{}/{} ({})", GP_USER_AGENT, client_version, default_os_version(client_os)),
  }
}

#[tauri::command]
pub fn get_connection_defaults() -> ConnectionDefaults {
  ConnectionDefaults {
    linux: make_os_defaults(&ClientOs::Linux),
    windows: make_os_defaults(&ClientOs::Windows),
    macos: make_os_defaults(&ClientOs::Mac),
  }
}

/// Maps the HIP report settings to the ConnectArgs source.
fn hip_source(submit_hip_report: Option<bool>, hip_script_path: Option<String>) -> HipSource {
  if submit_hip_report == Some(false) {
    return HipSource::Disabled;
  }
  match hip_script_path.filter(|p| !p.trim().is_empty()) {
    Some(path) => HipSource::UserScript { path },
    None => HipSource::Generated,
  }
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn get_prelogin(
  portal: String,
  certificate: Option<String>,
  sslkey: Option<String>,
  key_password: Option<String>,
  client_os: Option<String>,
  os_version: Option<String>,
  client_version: Option<String>,
  user_agent: Option<String>,
) -> Result<PreloginInfo, String> {
  let cert = CertConfig {
    certificate: certificate.filter(|s| !s.is_empty()),
    sslkey: sslkey.filter(|s| !s.is_empty()),
    key_password: key_password.filter(|s| !s.is_empty()),
  };
  let profile = OsProfileConfig {
    client_os,
    os_version,
    client_version,
    user_agent,
  };
  connect::get_prelogin(&portal, cert, profile).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn clear_credentials(state: State<'_, AppState>) -> Result<(), String> {
  *state.clean_next_auth.lock().await = true;
  Ok(())
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn connect_saml(
  portal: String,
  gateway: Option<String>,
  browser: Option<String>,
  reuse_auth_cookies: Option<bool>,
  use_external_browser: Option<bool>,
  certificate: Option<String>,
  sslkey: Option<String>,
  key_password: Option<String>,
  disable_ipv6: Option<bool>,
  no_dtls: Option<bool>,
  client_os: Option<String>,
  os_version: Option<String>,
  client_version: Option<String>,
  user_agent: Option<String>,
  vpnc_script: Option<String>,
  reconnect_timeout: Option<u32>,
  mtu: Option<u32>,
  submit_hip_report: Option<bool>,
  hip_script_path: Option<String>,
  hidpi: Option<bool>,
  app: AppHandle,
  state: State<'_, AppState>,
) -> Result<(), String> {
  // Fail fast before the long SAML browser flow if the service isn't ready
  if state.client.lock().await.is_none() {
    return Err("Not connected to gpservice".to_string());
  }

  let auth_executable = state.auth_executable.lock().await.clone();

  // Consume the explicit clear-credentials flag
  let force_clean = {
    let mut flag = state.clean_next_auth.lock().await;
    let v = *flag;
    *flag = false;
    v
  };

  // clean = true if explicitly requested OR if the user has disabled cookie reuse
  let clean = force_clean || !reuse_auth_cookies.unwrap_or(true);

  let default_browser = use_external_browser.unwrap_or(false);

  // External-browser mode → route through the isolated-window wrapper so
  // gpauth never kills the user's main browser
  let browser = if default_browser && browser.is_none() {
    Some(format!("{}/.local/bin/gp-auth-browser", std::env::var("HOME").unwrap_or_default()))
  } else {
    browser
  };
  // Absolute browser paths must exist (the wrapper ships with upstream
  // GlobalProtect-openconnect); fall back to the embedded auth window.
  let browser = browser.filter(|b| {
    if b.starts_with('/') && !std::path::Path::new(b).exists() {
      warn!("External browser wrapper {} is not installed; ignoring it", b);
      false
    } else {
      true
    }
  });
  info!("Starting SAML connect to portal: {}, clean={}, default_browser={}", portal, clean, default_browser);

  let cert = CertConfig {
    certificate: certificate.filter(|s| !s.is_empty()),
    sslkey: sslkey.filter(|s| !s.is_empty()),
    key_password: key_password.filter(|s| !s.is_empty()),
  };
  let profile = OsProfileConfig {
    client_os,
    os_version,
    client_version,
    user_agent,
  };
  let tunnel = TunnelConfig {
    disable_ipv6: disable_ipv6.unwrap_or(false),
    no_dtls: no_dtls.unwrap_or(false),
    vpnc_script,
    reconnect_timeout: reconnect_timeout.unwrap_or(300),
    mtu: mtu.unwrap_or(0),
    hip_source: hip_source(submit_hip_report, hip_script_path),
  };

  let saml_opts = connect::SamlOpts {
    browser,
    auth_executable,
    clean,
    default_browser,
    hidpi: hidpi.unwrap_or(false),
  };
  let opts = connect::ConnectOpts { cert, tunnel, profile };
  let cancellation = tokio_util::sync::CancellationToken::new();
  *state.connect_cancel.lock().await = Some(cancellation.clone());
  let ctx = connect::FlowCtx {
    app: &app,
    pick: GatewayPick::new(Arc::clone(&state.pending_gateway)),
    cancellation: &cancellation,
  };
  let result = connect::connect_saml(&portal, gateway.as_deref(), saml_opts, opts, ctx).await;
  *state.connect_cancel.lock().await = None;
  let (req, cached) = result.map_err(|e| e.to_string())?;

  let client = state.client.lock().await;
  let Some(ref client) = *client else {
    return Err("Not connected to gpservice".to_string());
  };

  client
    .send(WsRequest::Connect(Box::new(req)))
    .await
    .map_err(|e| e.to_string())?;

  // Only cache the session once the connect request was actually accepted
  *state.cached_auth.lock().await = Some(cached);

  info!("ConnectRequest sent to gpservice");
  Ok(())
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn connect_password(
  portal: String,
  gateway: Option<String>,
  username: String,
  password: String,
  certificate: Option<String>,
  sslkey: Option<String>,
  key_password: Option<String>,
  disable_ipv6: Option<bool>,
  no_dtls: Option<bool>,
  client_os: Option<String>,
  os_version: Option<String>,
  client_version: Option<String>,
  user_agent: Option<String>,
  vpnc_script: Option<String>,
  reconnect_timeout: Option<u32>,
  mtu: Option<u32>,
  submit_hip_report: Option<bool>,
  hip_script_path: Option<String>,
  app: AppHandle,
  state: State<'_, AppState>,
) -> Result<(), String> {
  info!("Starting password connect to portal: {}", portal);

  // Fail fast before the auth flow if the service isn't ready
  if state.client.lock().await.is_none() {
    return Err("Not connected to gpservice".to_string());
  }

  // Wipe this copy of the password when the command returns
  let password = zeroize::Zeroizing::new(password);

  let cert = CertConfig {
    certificate: certificate.filter(|s| !s.is_empty()),
    sslkey: sslkey.filter(|s| !s.is_empty()),
    key_password: key_password.filter(|s| !s.is_empty()),
  };
  let profile = OsProfileConfig {
    client_os,
    os_version,
    client_version,
    user_agent,
  };
  let tunnel = TunnelConfig {
    disable_ipv6: disable_ipv6.unwrap_or(false),
    no_dtls: no_dtls.unwrap_or(false),
    vpnc_script,
    reconnect_timeout: reconnect_timeout.unwrap_or(300),
    mtu: mtu.unwrap_or(0),
    hip_source: hip_source(submit_hip_report, hip_script_path),
  };

  let opts = connect::ConnectOpts { cert, tunnel, profile };
  let cancellation = tokio_util::sync::CancellationToken::new();
  *state.connect_cancel.lock().await = Some(cancellation.clone());
  let ctx = connect::FlowCtx {
    app: &app,
    pick: GatewayPick::new(Arc::clone(&state.pending_gateway)),
    cancellation: &cancellation,
  };
  let result = connect::connect_password(&portal, gateway.as_deref(), &username, password.as_str(), opts, ctx).await;
  *state.connect_cancel.lock().await = None;
  let (req, cached) = result.map_err(|e| e.to_string())?;

  let client = state.client.lock().await;
  let Some(ref client) = *client else {
    return Err("Not connected to gpservice".to_string());
  };

  client
    .send(WsRequest::Connect(Box::new(req)))
    .await
    .map_err(|e| e.to_string())?;

  // Only cache the session once the connect request was actually accepted
  *state.cached_auth.lock().await = Some(cached);

  info!("ConnectRequest sent to gpservice");
  Ok(())
}

#[tauri::command]
pub fn open_settings(app: AppHandle, section: Option<String>) -> Result<(), String> {
  if let Some(w) = app.get_webview_window("settings") {
    let _ = w.show();
    let _ = w.set_focus();
    if let Some(ref s) = section {
      let _ = app.emit_to("settings", "navigate-settings-section", s);
    }
    return Ok(());
  }

  let url = match section {
    Some(s) => {
      let safe: String = s
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(*c, '-' | '_'))
        .collect();
      format!("/?window=settings&section={}", safe)
    }
    None => "/?window=settings".to_string(),
  };

  WebviewWindowBuilder::new(&app, "settings", WebviewUrl::App(url.into()))
    .title("NusaProtect Settings")
    .inner_size(700.0, 560.0)
    .center()
    .resizable(false)
    .decorations(false)
    .build()
    .map_err(|e| e.to_string())?;

  Ok(())
}

#[tauri::command]
pub fn open_url(url: String) -> Result<(), String> {
  if !(url.starts_with("https://") || url.starts_with("http://") || url.starts_with("mailto:")) {
    return Err("Unsupported URL scheme".into());
  }

  // .status() (not .spawn()) so the delegated opener is reaped: xdg-open,
  // open and cmd /c start all return promptly after handing the URL off.
  #[cfg(target_os = "linux")]
  std::process::Command::new("xdg-open")
    .arg(&url)
    .status()
    .map_err(|e| e.to_string())?;
  #[cfg(target_os = "macos")]
  std::process::Command::new("open")
    .arg(&url)
    .status()
    .map_err(|e| e.to_string())?;
  #[cfg(target_os = "windows")]
  // rundll32 avoids `cmd /c start` mangling URLs containing `&`
  std::process::Command::new("rundll32")
    .args(["url.dll,FileProtocolHandler", &url])
    .status()
    .map_err(|e| e.to_string())?;
  Ok(())
}

#[tauri::command]
pub async fn persist_launch_settings(
  app: AppHandle,
  hidpi: bool,
  start_minimized: bool,
  symbolic_tray_icon: bool,
) -> Result<(), String> {
  let settings = LaunchSettings {
    hidpi,
    start_minimized,
    symbolic_tray_icon,
  };
  settings.save().map_err(|e| e.to_string())?;

  // Apply the tray-icon preference live so it takes effect without a restart.
  if let Some(tray) = app.tray_by_id("main-tray") {
    if symbolic_tray_icon {
      if let Some(icon) = crate::app::symbolic_icon(&app) {
        let _ = tray.set_icon(Some(icon));
      }
    } else if let Some(icon) = app.default_window_icon().cloned() {
      let _ = tray.set_icon(Some(icon));
    }
  }

  Ok(())
}

#[tauri::command]
pub async fn switch_gateway(gateway: String, app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
  let cached = state
    .cached_auth
    .lock()
    .await
    .clone()
    .ok_or_else(|| "No cached session — please reconnect normally".to_string())?;

  let cancellation = tokio_util::sync::CancellationToken::new();
  *state.connect_cancel.lock().await = Some(cancellation.clone());
  let ctx = connect::FlowCtx {
    app: &app,
    pick: GatewayPick::new(Arc::clone(&state.pending_gateway)),
    cancellation: &cancellation,
  };
  let result = connect::connect_with_credential(&cached, &gateway, ctx).await;
  *state.connect_cancel.lock().await = None;
  let req = result.map_err(|e| e.to_string())?;

  use gpapi::service::request::DisconnectRequest;
  info!("Switching gateway to {}: disconnecting current tunnel", gateway);

  // Subscribe before sending Disconnect so the state event can't be missed.
  let mut vpn_state_rx = state.vpn_state_tx.subscribe();
  {
    let client = state.client.lock().await;
    let Some(ref client) = *client else {
      return Err("Not connected to gpservice".to_string());
    };
    client
      .send(WsRequest::Disconnect(DisconnectRequest))
      .await
      .map_err(|e| e.to_string())?;
  }

  // Wait until the service reports the tunnel down instead of sleeping a
  // fixed delay; the 10s timeout is a safety net — proceed anyway. The client
  // lock is intentionally released across the wait so the io task can run.
  let wait_disconnected = async {
    loop {
      match vpn_state_rx.recv().await {
        Ok(VpnState::Disconnected) | Ok(VpnState::Failed(_)) => break,
        Ok(_) => continue,
        Err(broadcast::error::RecvError::Lagged(_)) => continue,
        Err(broadcast::error::RecvError::Closed) => break,
      }
    }
  };
  let _ = tokio::time::timeout(Duration::from_secs(10), wait_disconnected).await;

  let client = state.client.lock().await;
  let Some(ref client) = *client else {
    return Err("Not connected to gpservice".to_string());
  };
  client
    .send(WsRequest::Connect(Box::new(req)))
    .await
    .map_err(|e| e.to_string())?;

  info!("Gateway switch request sent");
  Ok(())
}

#[tauri::command]
pub fn quit_app(app: AppHandle) {
  app.exit(0);
}

#[tauri::command]
pub async fn cancel_connect(state: State<'_, AppState>) -> Result<(), String> {
  if let Some(token) = state.connect_cancel.lock().await.take() {
    token.cancel();
  }
  // Also release any pending gateway prompt so the flow unwinds
  if let Some(tx) = state.pending_gateway.lock().await.take() {
    drop(tx);
  }
  Ok(())
}

#[tauri::command]
pub async fn select_gateway(gateway: Option<String>, state: State<'_, AppState>) -> Result<(), String> {
  let mut pending = state.pending_gateway.lock().await;
  if let Some(tx) = pending.take() {
    if let Some(name) = gateway {
      let _ = tx.send(name);
    }
    // None → sender dropped without a value → connect flow is cancelled
  }
  Ok(())
}

#[tauri::command]
pub fn minimize_window(window: tauri::WebviewWindow) -> Result<(), String> {
  // Iconify doesn't work reliably for frameless skip-taskbar windows;
  // "minimize" here means hide to the system tray.
  window.hide().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn show_window(window: tauri::WebviewWindow) -> Result<(), String> {
  window.show().map_err(|e| e.to_string())?;
  window.set_focus().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn close_window(window: tauri::WebviewWindow) -> Result<(), String> {
  window.close().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn start_dragging(window: tauri::WebviewWindow) -> Result<(), String> {
  window.start_dragging().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn disconnect(state: State<'_, AppState>) -> Result<(), String> {
  use gpapi::service::request::DisconnectRequest;

  info!("Sending disconnect request");

  let client = state.client.lock().await;
  let Some(ref client) = *client else {
    return Err("Not connected to gpservice".to_string());
  };

  client
    .send(WsRequest::Disconnect(DisconnectRequest))
    .await
    .map_err(|e| e.to_string())?;

  Ok(())
}

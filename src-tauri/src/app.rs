use std::sync::Arc;
use std::time::{Duration, Instant};

use gpapi::service::{
  request::{DisconnectRequest, WsRequest},
  transport::SessionCredential,
  vpn_state::VpnState,
};
use log::{info, warn};
use tauri::{
  menu::{Menu, MenuItem},
  tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
  Emitter, Listener, Manager, RunEvent,
};
use tokio::sync::Mutex;

// Wildcard import needed so generate_handler! can find __cmd__ companion macros
#[allow(unused_imports)]
use crate::commands::*;
use crate::{commands::AppState, service_client::connect_to_service};

const TRAY_CLICK_WINDOW: Duration = Duration::from_millis(400);

#[derive(Default)]
struct TrayClickState {
  last: Option<Instant>,
  burst: u8,
  generation: u64,
}

fn format_duration(secs: u64) -> String {
  let h = secs / 3600;
  let m = (secs % 3600) / 60;
  let s = secs % 60;
  if h > 0 {
    format!("{:02}:{:02}:{:02}", h, m, s)
  } else {
    format!("{:02}:{:02}", m, s)
  }
}

#[derive(Clone, Copy, PartialEq)]
enum TrayStatus {
  Disconnected,
  Connecting,
  Connected,
  Disconnecting,
}

impl TrayStatus {
  fn tooltip(self, connected_at: Option<Instant>) -> String {
    match self {
      Self::Disconnected => "NusaProtect — Disconnected".to_string(),
      Self::Connecting => "NusaProtect — Connecting…".to_string(),
      Self::Disconnecting => "NusaProtect — Disconnecting…".to_string(),
      Self::Connected => match connected_at {
        Some(start) => format!(
          "NusaProtect — Connected {}",
          format_duration(start.elapsed().as_secs())
        ),
        None => "NusaProtect — Connected".to_string(),
      },
    }
  }
}

/// Show `window` above other windows and focus it. Plain `set_focus` is
/// ignored by Wayland compositors (and some X11 WMs) for a window that was
/// only hidden or buried — never minimized — so briefly flip always-on-top
/// to force the raise, then drop it back into normal stacking order.
#[cfg(desktop)]
pub(crate) fn reveal_window<R: tauri::Runtime>(window: &tauri::WebviewWindow<R>) -> Result<(), String> {
  let _ = window.unminimize();
  window.show().map_err(|e| e.to_string())?;
  let _ = window.set_always_on_top(true);
  let _ = window.set_focus();
  let _ = window.set_always_on_top(false);
  Ok(())
}

#[cfg(desktop)]
fn show_main_window<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<(), String> {
  if let Some(window) = app.get_webview_window("main") {
    reveal_window(&window)
  } else {
    Err("Main window not found".to_string())
  }
}

#[cfg(desktop)]
fn toggle_main_window<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<(), String> {
  if let Some(window) = app.get_webview_window("main") {
    if !window.is_visible().unwrap_or(true) {
      // Window was fully hidden (e.g. started with --minimized): restore it.
      reveal_window(&window)?;
    } else if window.is_focused().unwrap_or(false) {
      // Window is visible and focused: minimize to tray.
      let _ = window.minimize();
    } else {
      // Window is minimized or behind other windows: bring it to front.
      reveal_window(&window)?;
    }
    Ok(())
  } else {
    Err("Main window not found".to_string())
  }
}

/// Converts the default window icon into a monochrome "symbolic" variant
/// (BT.601 luminance, alpha preserved) for tray themes that expect one.
/// Returns None when no default icon is available.
pub(crate) fn symbolic_icon(app: &tauri::AppHandle) -> Option<tauri::image::Image<'static>> {
  let icon = app.default_window_icon()?;
  let width = icon.width();
  let height = icon.height();
  let rgba: Vec<u8> = icon
    .rgba()
    .chunks_exact(4)
    .flat_map(|px| {
      let luma = (0.299 * px[0] as f32 + 0.587 * px[1] as f32 + 0.114 * px[2] as f32) as u8;
      [luma, luma, luma, px[3]]
    })
    .collect();
  Some(tauri::image::Image::new_owned(rgba, width, height))
}

pub struct App {
  credential: Arc<SessionCredential>,
  minimized: bool,
  symbolic_tray_icon: bool,
}

impl App {
  pub fn new(credential: Arc<SessionCredential>, minimized: bool, symbolic_tray_icon: bool) -> Self {
    Self {
      credential,
      minimized,
      symbolic_tray_icon,
    }
  }

  pub fn run(self) -> anyhow::Result<()> {
    let credential = self.credential;
    let minimized = self.minimized;
    let symbolic_tray_icon = self.symbolic_tray_icon;
    let state = AppState::new();

    let app = tauri::Builder::default()
      .manage(state)
      .invoke_handler(tauri::generate_handler![
        get_connection_defaults,
        get_prelogin,
        connect_saml,
        connect_password,
        select_gateway,
        cancel_connect,
        switch_gateway,
        disconnect,
        logout,
        clear_dns,
        quit_app,
        minimize_window,
        show_window,
        close_window,
        start_dragging,
        open_settings,
        open_url,

        persist_launch_settings,
      ])
      .setup(move |app| {
        let app_handle = app.handle().clone();

        // Clone the Arc fields from managed state so they can be moved
        // into 'static closures without lifetime issues.
        let client_store = Arc::clone(&app.state::<AppState>().client);
        let auth_exec_store = Arc::clone(&app.state::<AppState>().auth_executable);

        #[cfg(desktop)]
        {
          let vpn_status: Arc<Mutex<TrayStatus>> = Arc::new(Mutex::new(TrayStatus::Disconnected));
          let connected_at: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));

          let tray_toggle_item = MenuItem::with_id(
            &app_handle,
            "tray-toggle-window",
            "Toggle Window",
            true,
            None::<&str>,
          )?;
          let tray_settings_item = MenuItem::with_id(
            &app_handle,
            "tray-open-settings",
            "Settings",
            true,
            None::<&str>,
          )?;
          let tray_connect_item = MenuItem::with_id(
            &app_handle,
            "tray-connect-disconnect",
            "Connect",
            true,
            None::<&str>,
          )?;
          let tray_quit_item = MenuItem::with_id(
            &app_handle,
            "tray-quit",
            "Quit",
            true,
            None::<&str>,
          )?;

          let tray_menu = Menu::with_items(
            &app_handle,
            &[
              &tray_toggle_item,
              &tray_settings_item,
              &tray_connect_item,
              &tray_quit_item,
            ],
          )?;

          let vpn_status_menu = Arc::clone(&vpn_status);
          let client_store_menu = Arc::clone(&client_store);

          let mut tray_builder = TrayIconBuilder::with_id("main-tray")
            .tooltip("NusaProtect")
            .menu(&tray_menu)
            .show_menu_on_left_click(false);

          let tray_icon = if symbolic_tray_icon {
            symbolic_icon(&app_handle)
          } else {
            app_handle.default_window_icon().cloned()
          };
          if let Some(icon) = tray_icon {
            tray_builder = tray_builder.icon(icon);
          } else {
            warn!("No default window icon available for tray icon");
          }

          // Distinguish single- from double-click like Flameshot does: hold
          // the action for ~400 ms; a second activate event inside the window
          // upgrades it to a double-click (always show) while a lone event
          // resolves as a toggle. A double-click can also arrive as a burst
          // of Activate/SecondaryActivate events, which the same window
          // collapses into the double-click resolution.
          let tray_click_state = Arc::new(std::sync::Mutex::new(TrayClickState::default()));
          let tray = tray_builder
            .on_menu_event(move |app, event| match event.id().as_ref() {
              "tray-toggle-window" => {
                if let Err(e) = toggle_main_window(app) {
                  warn!("Failed to toggle main window from tray menu: {}", e);
                }
              }
              "tray-open-settings" => {
                let _ = open_settings(app.clone(), None);
              }
              "tray-connect-disconnect" => {
                let vs = Arc::clone(&vpn_status_menu);
                let client = Arc::clone(&client_store_menu);
                let app_clone = app.clone();
                tauri::async_runtime::spawn(async move {
                  let is_connected = *vs.lock().await == TrayStatus::Connected;
                  if is_connected {
                    let guard = client.lock().await;
                    if let Some(c) = guard.as_ref() {
                      if let Err(e) = c.send(WsRequest::Disconnect(DisconnectRequest)).await {
                        warn!("Failed to disconnect from tray: {}", e);
                      }
                    } else {
                      warn!("Disconnect requested from tray, but no client is connected");
                    }
                  } else {
                    let _ = app_clone.emit_to("main", "tray-connect-requested", ());
                  }
                });
              }
              "tray-quit" => {
                app.exit(0);
              }
              _ => {}
            })
            .on_tray_icon_event(move |tray, event| {
              // Activate-class events differ per DE: KDE sends Activate on a
              // single left click, many GNOME setups map double-click to
              // Activate or SecondaryActivate, Windows reports DoubleClick.
              let activates = matches!(
                event,
                TrayIconEvent::Click {
                  button: MouseButton::Left | MouseButton::Middle,
                  button_state: MouseButtonState::Up,
                  ..
                } | TrayIconEvent::DoubleClick {
                  button: MouseButton::Left,
                  ..
                }
              );
              if !activates {
                return;
              }
              let Ok(mut state) = tray_click_state.lock() else {
                return;
              };
              let now = Instant::now();
              state.burst = if state
                .last
                .is_some_and(|last| now.duration_since(last) < TRAY_CLICK_WINDOW)
              {
                state.burst.saturating_add(1)
              } else {
                1
              };
              state.last = Some(now);
              state.generation += 1;
              let generation = state.generation;
              let burst = state.burst;
              drop(state);

              let app = tray.app_handle().clone();
              let click_state = Arc::clone(&tray_click_state);
              tauri::async_runtime::spawn(async move {
                tokio::time::sleep(TRAY_CLICK_WINDOW).await;
                let superseded = click_state
                  .lock()
                  .map(|state| state.generation != generation)
                  .unwrap_or(true);
                if superseded {
                  return;
                }
                let result = if burst >= 2 {
                  show_main_window(&app)
                } else {
                  toggle_main_window(&app)
                };
                if let Err(e) = result {
                  warn!("Failed to handle tray activation: {}", e);
                }
              });
            })
            .build(&app_handle)?;

          let _ = tray;

          let vpn_status_listener = Arc::clone(&vpn_status);
          let connected_at_listener = Arc::clone(&connected_at);
          let tray_connect_item_listener = tray_connect_item.clone();
          let app_handle_listener = app_handle.clone();
          app.listen_any("vpn-state", move |event| {
            let item = tray_connect_item_listener.clone();
            let vs = Arc::clone(&vpn_status_listener);
            let cat = Arc::clone(&connected_at_listener);
            let app_handle = app_handle_listener.clone();
            if let Ok(state) = serde_json::from_str::<VpnState>(event.payload()) {
              let new_status = match state {
                VpnState::Connected(_) => TrayStatus::Connected,
                VpnState::Connecting(_) | VpnState::ReconnectRequired(_) => TrayStatus::Connecting,
                VpnState::Disconnecting => TrayStatus::Disconnecting,
                VpnState::Disconnected | VpnState::Failed(_) => TrayStatus::Disconnected,
              };
              tauri::async_runtime::spawn(async move {
                let prev = {
                  let mut guard = vs.lock().await;
                  let prev = *guard;
                  *guard = new_status;
                  prev
                };

                let at = if new_status == TrayStatus::Connected && prev != TrayStatus::Connected {
                  let now = Instant::now();
                  *cat.lock().await = Some(now);
                  Some(now)
                } else if new_status != TrayStatus::Connected {
                  *cat.lock().await = None;
                  None
                } else {
                  *cat.lock().await
                };

                let label = if new_status == TrayStatus::Connected { "Disconnect" } else { "Connect" };
                if let Err(e) = item.set_text(label) {
                  warn!("Failed to update tray item text: {}", e);
                }

                if let Some(tray) = app_handle.tray_by_id("main-tray") {
                  let _ = tray.set_tooltip(Some(new_status.tooltip(at).as_str()));
                }
              });
            }
          });

          let app_handle_timer = app_handle.clone();
          let vpn_status_timer = Arc::clone(&vpn_status);
          let connected_at_timer = Arc::clone(&connected_at);
          tauri::async_runtime::spawn(async move {
            loop {
              tokio::time::sleep(std::time::Duration::from_secs(1)).await;
              let status = *vpn_status_timer.lock().await;
              // Only re-render every second while connected; other states are
              // already updated immediately by the vpn-state listener.
              if status == TrayStatus::Connected {
                let at = *connected_at_timer.lock().await;
                let tooltip = status.tooltip(at);
                if let Some(tray) = app_handle_timer.tray_by_id("main-tray") {
                  let _ = tray.set_tooltip(Some(tooltip.as_str()));
                }
              }
            }
          });
        }

        if let Some(window) = app.get_webview_window("main") {
          let window_clone = window.clone();
          window.on_window_event(move |event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
              api.prevent_close();
              let _ = window_clone.emit("window-close-requested", ());
            }
          });
        }

        // Connect to gpservice in the background; on failure or disconnect,
        // keep retrying with exponential backoff. The io task emits
        // "service-disconnected" when the socket drops, then we reconnect here.
        tauri::async_runtime::spawn(async move {
          let mut first_failure = true;
          let mut backoff = Duration::from_secs(1);
          loop {
            match connect_to_service(Arc::clone(&credential), app_handle.clone(), Arc::clone(&client_store)).await {
              Ok((client, io_done, vpn_state)) => {
                info!("Connected to gpservice");
                *client_store.lock().await = Some(client);
                // Carry the attach snapshot: the "vpn-state" emit inside
                // connect_to_service can race past frontend listener
                // registration, so this is the reliable delivery channel.
                let _ = app_handle.emit("service-connected", &vpn_state);
                first_failure = true;
                backoff = Duration::from_secs(1);
                // Wait until the io task ends (socket closed/error), then reconnect.
                let _ = io_done.await;
              }
              Err(e) => {
                if first_failure {
                  let _ = app_handle.emit("service-error", e.to_string());
                  first_failure = false;
                }
                warn!("Failed to connect to gpservice: {} (retrying in {:?})", e, backoff);
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(30));
              }
            }
          }
        });

        // Broadcast every VPN state on the internal channel too; internal
        // consumers (e.g. switch_gateway waiting for Disconnected) subscribe
        // to it rather than listening to frontend events.
        let vpn_state_tx = app.state::<AppState>().vpn_state_tx.clone();
        app.listen_any("vpn-state", move |event| {
          if let Ok(state) = serde_json::from_str::<VpnState>(event.payload()) {
            let _ = vpn_state_tx.send(state);
          }
        });

        // Cache the auth_executable path whenever VpnEnv is received
        let auth_exec_store2 = Arc::clone(&auth_exec_store);
        app.listen_any("vpn-env", move |event| {
          use gpapi::service::vpn_env::VpnEnv;
          if let Ok(env) = serde_json::from_str::<VpnEnv>(event.payload()) {
            let store = Arc::clone(&auth_exec_store2);
            tauri::async_runtime::spawn(async move {
              *store.lock().await = Some(env.auth_executable);
            });
          }
        });

        if minimized {
          if let Some(win) = app.get_webview_window("main") {
            let _ = win.hide();
          }
        }

        Ok(())
      })
      .build(tauri::generate_context!())?;

    app.run(move |_app_handle, event| {
      if let RunEvent::ExitRequested { api, code, .. } = event {
        if code.is_none() {
          api.prevent_exit();
        }
      }
    });

    Ok(())
  }
}

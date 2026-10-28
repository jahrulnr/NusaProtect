use std::{env, fs, io, path::PathBuf};

use serde::{Deserialize, Serialize};

/// Settings the backend needs at launch time, before the webview (and its
/// localStorage) is readable. Persisted by the `persist_launch_settings`
/// command; the frontend mirrors the same values in localStorage for the UI.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LaunchSettings {
  pub hidpi: bool,
  pub start_minimized: bool,
  pub symbolic_tray_icon: bool,
}

/// `$XDG_CONFIG_HOME/io.github.jahrulnr.nusaprotect/launch-settings.json`
/// (falls back to `~/.config` when XDG_CONFIG_HOME is unset or empty).
pub fn launch_settings_path() -> PathBuf {
  let config_home = env::var("XDG_CONFIG_HOME")
    .ok()
    .filter(|v| !v.is_empty())
    .unwrap_or_else(|| format!("{}/.config", env::var("HOME").unwrap_or_default()));
  PathBuf::from(config_home).join("io.github.jahrulnr.nusaprotect/launch-settings.json")
}

impl LaunchSettings {
  /// Reads the settings file; any error (missing file, bad JSON) yields the
  /// defaults.
  pub fn load() -> Self {
    fs::read_to_string(launch_settings_path())
      .ok()
      .and_then(|data| serde_json::from_str(&data).ok())
      .unwrap_or_default()
  }

  /// Writes the settings file, creating the config directory if needed.
  pub fn save(&self) -> io::Result<()> {
    let path = launch_settings_path();
    if let Some(parent) = path.parent() {
      fs::create_dir_all(parent)?;
    }
    fs::write(&path, serde_json::to_string_pretty(self).unwrap_or_default())
  }
}

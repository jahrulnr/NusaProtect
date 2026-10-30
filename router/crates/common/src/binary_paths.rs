use std::{
  env,
  path::{Path, PathBuf},
};

#[cfg(target_os = "macos")]
mod macos;

#[cfg(all(
  not(debug_assertions),
  any(target_os = "linux", target_os = "freebsd", target_os = "openbsd")
))]
use crate::constants::GP_DOWNLOADED_GUI_BINARY;
#[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd"))]
use crate::constants::GP_HIP_SCRIPT_INSTALLER_BINARY;
#[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd"))]
use crate::constants::GP_VPNC_SCRIPT_INSTALLER_BINARY;
use crate::constants::{GP_AUTH_BINARY, GP_CLIENT_BINARY, GP_GUI_BINARY, GP_GUI_HELPER_BINARY, GP_SERVICE_BINARY};

pub fn gpclient() -> PathBuf {
  resolve(&["NP_CLIENT_BINARY", "GP_CLIENT_BINARY"], "npclient", GP_CLIENT_BINARY)
}

pub fn gpservice() -> PathBuf {
  resolve(&["NP_SERVICE_BINARY", "GP_SERVICE_BINARY"], "npservice", GP_SERVICE_BINARY)
}

#[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd"))]
pub fn gp_vpnc_script_installer() -> PathBuf {
  resolve(
    &["NP_VPNC_SCRIPT_INSTALLER_BINARY", "GP_VPNC_SCRIPT_INSTALLER_BINARY"],
    "np-vpnc-script-installer",
    GP_VPNC_SCRIPT_INSTALLER_BINARY,
  )
}

#[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd"))]
pub fn gp_hip_script_installer() -> PathBuf {
  resolve(
    &["NP_HIP_SCRIPT_INSTALLER_BINARY", "GP_HIP_SCRIPT_INSTALLER_BINARY"],
    "np-hip-script-installer",
    GP_HIP_SCRIPT_INSTALLER_BINARY,
  )
}

pub fn gpauth() -> PathBuf {
  resolve(&["NP_AUTH_BINARY", "GP_AUTH_BINARY"], "npauth", GP_AUTH_BINARY)
}

pub fn gpgui() -> PathBuf {
  resolve(&["NP_GUI_BINARY", "GP_GUI_BINARY"], "npgui", GP_GUI_BINARY)
}

pub fn gpgui_update_target() -> PathBuf {
  #[cfg(all(
    not(debug_assertions),
    any(target_os = "linux", target_os = "freebsd", target_os = "openbsd")
  ))]
  {
    PathBuf::from(GP_DOWNLOADED_GUI_BINARY)
  }

  #[cfg(any(
    debug_assertions,
    not(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd"))
  ))]
  {
    gpgui()
  }
}

pub fn gpgui_helper() -> PathBuf {
  resolve(
    &["NP_GUI_HELPER_BINARY", "GP_GUI_HELPER_BINARY"],
    "npgui-helper",
    GP_GUI_HELPER_BINARY,
  )
}

fn resolve(env_keys: &[&str], binary_name: &str, default_path: &str) -> PathBuf {
  for env_key in env_keys {
    if let Some(value) = env::var_os(env_key).filter(|value| !value.is_empty()) {
      return PathBuf::from(value);
    }
  }
  sibling_binary(binary_name).unwrap_or_else(|| PathBuf::from(default_path))
}

fn sibling_binary(binary_name: &str) -> Option<PathBuf> {
  let current_exe = env::current_exe().ok()?;
  sibling_binary_for_executable(&current_exe, binary_name)
}

fn sibling_binary_for_executable(executable: &Path, binary_name: &str) -> Option<PathBuf> {
  let executable = executable.canonicalize().ok()?;
  let bin_dir = executable.parent()?;
  let binary = bin_dir.join(binary_name);
  if is_file(&binary) {
    return Some(binary);
  }
  if matches!(binary_name, "np-vpnc-script-installer" | "np-hip-script-installer") {
    let binary = bin_dir.parent()?.join("libexec/npclient").join(binary_name);
    if is_file(&binary) {
      return Some(binary);
    }
  }
  #[cfg(target_os = "macos")]
  if let Some(binary) = macos::helper_path(&executable, binary_name)
    && is_file(&binary)
  {
    return Some(binary);
  }
  None
}

fn is_file(path: &Path) -> bool {
  path.is_file()
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn script_installers_follow_the_installed_prefix() {
    let directory = tempfile::tempdir().unwrap();
    let bin = directory.path().join("bin");
    let libexec = directory.path().join("libexec/npclient");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(&libexec).unwrap();
    std::fs::write(bin.join("npclient"), "").unwrap();
    for name in ["np-vpnc-script-installer", "np-hip-script-installer"] {
      std::fs::write(libexec.join(name), "").unwrap();
      assert_eq!(
        sibling_binary_for_executable(&bin.join("npclient"), name),
        Some(libexec.canonicalize().unwrap().join(name))
      );
    }
  }

  #[test]
  #[cfg(target_os = "macos")]
  fn desktop_resolves_helpers_in_the_actual_app_bundle_layout() {
    assert_eq!(
      macos::helper_path(
        Path::new("/Applications/GP Connect.app/Contents/MacOS/npgui"),
        "npclient"
      ),
      Some(PathBuf::from("/Applications/GP Connect.app/Contents/Helpers/npclient"))
    );
    assert!(macos::helper_path(Path::new("/usr/local/bin/npgui"), "npclient").is_none());
    assert!(macos::helper_path(Path::new("/tmp/Contents/MacOS/npgui"), "npclient").is_none());
  }

  #[test]
  #[cfg(unix)]
  fn symlinked_executable_resolves_its_bundle_paths() {
    let directory = tempfile::tempdir().unwrap();
    let helpers = directory.path().join("GP Connect.app/Contents/Helpers");
    let bin = directory.path().join("bin");
    std::fs::create_dir_all(&helpers).unwrap();
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(helpers.join("npclient"), "").unwrap();
    std::fs::write(helpers.join("npauth"), "").unwrap();
    std::os::unix::fs::symlink(helpers.join("npclient"), bin.join("npclient")).unwrap();

    assert_eq!(
      sibling_binary_for_executable(&bin.join("npclient"), "npauth"),
      Some(helpers.canonicalize().unwrap().join("npauth"))
    );
  }

  #[test]
  fn default_path_is_used_when_no_sibling_exists() {
    let path = resolve(&["NP_TEST_BINARY"], "missing-np-test-binary", "/usr/bin/np-test");

    assert_eq!(path, PathBuf::from("/usr/bin/np-test"));
  }
}

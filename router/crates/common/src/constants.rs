pub const GP_USER_AGENT: &str = "PAN GlobalProtect";
pub const GP_CLIENT_VERSION_LINUX: &str = "6.3.3-619";
pub const GP_CLIENT_VERSION_WINDOWS: &str = "6.3.3-650";
pub const GP_CLIENT_VERSION_MACOS: &str = "6.3.3-915";
pub const GP_SERVICE_LOCK_FILE: &str = "/var/run/npservice.lock";
#[cfg(target_os = "linux")]
pub const GP_VPNC_SCRIPT_INSTALLER_BINARY: &str = "/usr/libexec/npclient/np-vpnc-script-installer";
#[cfg(target_os = "linux")]
pub const GP_HIP_SCRIPT_INSTALLER_BINARY: &str = "/usr/libexec/npclient/np-hip-script-installer";
#[cfg(any(target_os = "freebsd", target_os = "openbsd"))]
pub const GP_HIP_SCRIPT_INSTALLER_BINARY: &str = "/usr/local/libexec/npclient/np-hip-script-installer";
#[cfg(target_os = "linux")]
pub const INSTALLED_VPNC_SCRIPT: &str = "/var/lib/npclient/scripts/vpnc-script";
#[cfg(target_os = "linux")]
pub const INSTALLED_VPNC_SCRIPT_METADATA: &str = "/var/lib/npclient/scripts/vpnc-script.metadata";
#[cfg(any(target_os = "freebsd", target_os = "openbsd"))]
pub const GP_VPNC_SCRIPT_INSTALLER_BINARY: &str = "/usr/local/libexec/npclient/np-vpnc-script-installer";
#[cfg(any(target_os = "freebsd", target_os = "openbsd"))]
pub const INSTALLED_VPNC_SCRIPT: &str = "/var/db/npclient/scripts/vpnc-script";
#[cfg(any(target_os = "freebsd", target_os = "openbsd"))]
pub const INSTALLED_VPNC_SCRIPT_METADATA: &str = "/var/db/npclient/scripts/vpnc-script.metadata";
pub const MAX_VPNC_SCRIPT_SIZE: usize = 1024 * 1024;
pub const MAX_HIP_SCRIPT_SIZE: usize = 1024 * 1024;

// Release binaries - macOS (Apple Silicon Homebrew)
#[cfg(all(not(debug_assertions), target_os = "macos", target_arch = "aarch64"))]
pub const GP_CLIENT_BINARY: &str = "/opt/homebrew/bin/npclient";
#[cfg(all(not(debug_assertions), target_os = "macos", target_arch = "aarch64"))]
pub const GP_SERVICE_BINARY: &str = "/opt/homebrew/bin/npservice";
#[cfg(all(not(debug_assertions), target_os = "macos", target_arch = "aarch64"))]
pub const GP_GUI_BINARY: &str = "/opt/homebrew/bin/npgui";
#[cfg(all(not(debug_assertions), target_os = "macos", target_arch = "aarch64"))]
pub const GP_GUI_HELPER_BINARY: &str = "/opt/homebrew/bin/npgui-helper";
#[cfg(all(not(debug_assertions), target_os = "macos", target_arch = "aarch64"))]
pub const GP_AUTH_BINARY: &str = "/opt/homebrew/bin/npauth";

// Release binaries - macOS (Intel Homebrew)
#[cfg(all(not(debug_assertions), target_os = "macos", target_arch = "x86_64"))]
pub const GP_CLIENT_BINARY: &str = "/usr/local/bin/npclient";
#[cfg(all(not(debug_assertions), target_os = "macos", target_arch = "x86_64"))]
pub const GP_SERVICE_BINARY: &str = "/usr/local/bin/npservice";
#[cfg(all(not(debug_assertions), target_os = "macos", target_arch = "x86_64"))]
pub const GP_GUI_BINARY: &str = "/usr/local/bin/npgui";
#[cfg(all(not(debug_assertions), target_os = "macos", target_arch = "x86_64"))]
pub const GP_GUI_HELPER_BINARY: &str = "/usr/local/bin/npgui-helper";
#[cfg(all(not(debug_assertions), target_os = "macos", target_arch = "x86_64"))]
pub const GP_AUTH_BINARY: &str = "/usr/local/bin/npauth";

// Release binaries - BSD package layout
#[cfg(all(not(debug_assertions), any(target_os = "freebsd", target_os = "openbsd")))]
pub const GP_CLIENT_BINARY: &str = "/usr/local/bin/npclient";
#[cfg(all(not(debug_assertions), any(target_os = "freebsd", target_os = "openbsd")))]
pub const GP_SERVICE_BINARY: &str = "/usr/local/bin/npservice";
#[cfg(all(not(debug_assertions), any(target_os = "freebsd", target_os = "openbsd")))]
pub const GP_GUI_BINARY: &str = "/usr/local/bin/npgui";
#[cfg(any(target_os = "freebsd", target_os = "openbsd"))]
pub const GP_DOWNLOADED_GUI_BINARY: &str = "/usr/local/libexec/npclient/npgui";
#[cfg(all(not(debug_assertions), any(target_os = "freebsd", target_os = "openbsd")))]
pub const GP_GUI_HELPER_BINARY: &str = "/usr/local/bin/npgui-helper";
#[cfg(all(not(debug_assertions), any(target_os = "freebsd", target_os = "openbsd")))]
pub const GP_AUTH_BINARY: &str = "/usr/local/bin/npauth";

// Release binaries - Linux
#[cfg(all(
  not(debug_assertions),
  not(any(target_os = "macos", target_os = "freebsd", target_os = "openbsd"))
))]
pub const GP_CLIENT_BINARY: &str = "/usr/bin/npclient";
#[cfg(all(
  not(debug_assertions),
  not(any(target_os = "macos", target_os = "freebsd", target_os = "openbsd"))
))]
pub const GP_SERVICE_BINARY: &str = "/usr/bin/npservice";
#[cfg(all(
  not(debug_assertions),
  not(any(target_os = "macos", target_os = "freebsd", target_os = "openbsd"))
))]
pub const GP_GUI_BINARY: &str = "/usr/bin/npgui";
#[cfg(target_os = "linux")]
pub const GP_DOWNLOADED_GUI_BINARY: &str = "/var/lib/npclient/npgui";
#[cfg(all(
  not(debug_assertions),
  not(any(target_os = "macos", target_os = "freebsd", target_os = "openbsd"))
))]
pub const GP_GUI_HELPER_BINARY: &str = "/usr/bin/npgui-helper";
#[cfg(all(
  not(debug_assertions),
  not(any(target_os = "macos", target_os = "freebsd", target_os = "openbsd"))
))]
pub const GP_AUTH_BINARY: &str = "/usr/bin/npauth";

// Debug binaries are set in build.rs via environment variables
#[cfg(debug_assertions)]
pub const GP_CLIENT_BINARY: &str = env!("GP_CLIENT_BINARY");
#[cfg(debug_assertions)]
pub const GP_SERVICE_BINARY: &str = env!("GP_SERVICE_BINARY");
#[cfg(debug_assertions)]
pub const GP_GUI_BINARY: &str = env!("GP_GUI_BINARY");
#[cfg(debug_assertions)]
pub const GP_GUI_HELPER_BINARY: &str = env!("GP_GUI_HELPER_BINARY");
#[cfg(debug_assertions)]
pub const GP_AUTH_BINARY: &str = env!("GP_AUTH_BINARY");

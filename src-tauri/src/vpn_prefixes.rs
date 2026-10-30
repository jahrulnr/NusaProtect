//! Turn the settings field into a vpnc wrapper that exports `GP_VPN_PREFIXES`.
//! OpenConnect builds the script environment itself, so a value set in the GUI
//! process would never reach `allowroute`. The wrapper is the script OpenConnect runs.

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const DEFAULT_PREFIX: &str = "10.0.0.0/8";
const STOCK_SCRIPTS: &[&str] = &[
  "/etc/vpnc/vpnc-script",
  "/usr/share/vpnc-scripts/vpnc-script",
  "/usr/sbin/vpnc-script",
  "/usr/local/sbin/vpnc-script",
];

pub fn fork_vpnc_script() -> PathBuf {
  PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../router/packaging/files/usr/libexec/gpclient/vpnc-script")
}

pub fn normalize_prefixes(raw: &str) -> Result<String, String> {
  let raw = raw.trim();
  let raw = if raw.is_empty() { DEFAULT_PREFIX } else { raw };
  let mut out = Vec::new();
  for part in raw.split(|c: char| c == ',' || c.is_whitespace()) {
    if part.is_empty() {
      continue;
    }
    if !is_cidr(part) {
      return Err(format!("VPN prefix '{part}' is not a CIDR like 10.0.0.0/8"));
    }
    out.push(part);
  }
  if out.is_empty() {
    return Err("VPN prefix list is empty".to_string());
  }
  Ok(out.join(" "))
}

pub fn choose_script(configured: Option<&str>, fork: &str, fork_exists: bool) -> String {
  let configured = configured.map(str::trim).filter(|s| !s.is_empty());
  match configured {
    Some(path) if fork_exists && STOCK_SCRIPTS.contains(&path) => fork.to_string(),
    Some(path) => path.to_string(),
    None if fork_exists => fork.to_string(),
    None => STOCK_SCRIPTS[0].to_string(),
  }
}

pub fn wrapper_body(prefixes: &str, inner_script: &str) -> String {
  format!(
    "#!/bin/sh\nexport GP_VPN_PREFIXES={}\nexec {} \"$@\"\n",
    shell_quote(prefixes),
    shell_quote(inner_script),
  )
}

pub fn prepare(configured_script: Option<&str>, prefixes: Option<&str>) -> Result<String, String> {
  let fork = fork_vpnc_script();
  let fork_s = fork.to_string_lossy();
  let inner = choose_script(configured_script, fork_s.as_ref(), fork.is_file());
  let prefixes = normalize_prefixes(prefixes.unwrap_or(""))?;
  let dir = config_dir()?;
  fs::create_dir_all(&dir).map_err(|err| format!("cannot create {}: {err}", dir.display()))?;
  let path = dir.join("vpnc-allowroute.sh");
  let body = wrapper_body(&prefixes, &inner);
  write_executable(&path, &body).map_err(|err| format!("cannot write {}: {err}", path.display()))?;
  Ok(path.to_string_lossy().into_owned())
}

fn config_dir() -> Result<PathBuf, String> {
  let home = std::env::var("HOME").map_err(|_| "HOME is not set".to_string())?;
  Ok(PathBuf::from(home).join(".config/io.github.jahrulnr.nusaprotect"))
}

fn write_executable(path: &Path, body: &str) -> io::Result<()> {
  fs::write(path, body)?;
  let mut perms = fs::metadata(path)?.permissions();
  perms.set_mode(0o755);
  fs::set_permissions(path, perms)
}

fn shell_quote(text: &str) -> String {
  format!("'{}'", text.replace('\'', "'\\''"))
}

fn is_cidr(text: &str) -> bool {
  let Some((addr, len)) = text.split_once('/') else {
    return false;
  };
  let Ok(len) = len.parse::<u8>() else {
    return false;
  };
  match addr.parse::<std::net::IpAddr>() {
    Ok(std::net::IpAddr::V4(_)) => len <= 32,
    Ok(std::net::IpAddr::V6(_)) => len <= 128,
    Err(_) => false,
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn empty_prefixes_use_the_default() {
    assert_eq!(normalize_prefixes("").unwrap(), "10.0.0.0/8");
    assert_eq!(normalize_prefixes("  ").unwrap(), "10.0.0.0/8");
  }

  #[test]
  fn joins_cidr_list() {
    assert_eq!(normalize_prefixes("10.0.0.0/8, 192.168.9.0/24").unwrap(), "10.0.0.0/8 192.168.9.0/24");
  }

  #[test]
  fn rejects_shell_metacharacters() {
    assert!(normalize_prefixes("10.0.0.0/8;touch /tmp/x").is_err());
    assert!(normalize_prefixes("not-a-cidr").is_err());
  }

  #[test]
  fn stock_script_is_replaced_when_the_fork_exists() {
    let chosen = choose_script(Some("/etc/vpnc/vpnc-script"), "/opt/fork/vpnc-script", true);
    assert_eq!(chosen, "/opt/fork/vpnc-script");
  }

  #[test]
  fn custom_script_is_kept() {
    let chosen = choose_script(Some("/opt/mine/vpnc-script"), "/opt/fork/vpnc-script", true);
    assert_eq!(chosen, "/opt/mine/vpnc-script");
  }

  #[test]
  fn wrapper_exports_prefixes_and_execs_the_inner_script() {
    let body = wrapper_body("10.0.0.0/8", "/opt/fork/vpnc-script");
    assert!(body.contains("export GP_VPN_PREFIXES='10.0.0.0/8'\n"));
    assert!(body.contains("exec '/opt/fork/vpnc-script' \"$@\"\n"));
  }
}

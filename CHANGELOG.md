# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

`VERSION` is the NusaProtect package version. The GlobalProtect client
version in `src-tauri/Cargo.toml` stays aligned with the portal client
version and is not this package version.

## [Unreleased]

## [0.1.3] - 2026-11-05

### Changed

- Dependency updates. Rust: `tauri` 2.12.4 → 2.12.5, `anyhow` 1.0.102 →
  1.0.104, `clap` 4.6.0 → 4.6.1, `netdev` 0.46.3 → 0.47.0,
  `thiserror` 2.0.18 → 2.0.21, `zeroize` 1.8.2 → 1.9.1.
  npm: `vite` 7.3.5 → 8.3.5,
  `@types/node` 26.6.5 → 26.6.6, `@types/react-dom` 19.2.3 → 19.3.0.

## [0.1.2] - 2026-11-02

### Added

- Double-clicking the tray icon now opens the main window directly.
  Activate-class events (Activate, SecondaryActivate, DoubleClick) are
  held ~400 ms so a second event can upgrade the gesture to a
  double-click — which always shows the window — while a lone event
  resolves as the usual show/hide toggle, matching Flameshot-style tray
  behavior across DEs.
- DNS-derived tunnel routes now expire and are re-resolved. A name that fell
  back to VPN DNS gets a TTL (default 300 s, configurable in Settings →
  Connection → DNS Route TTL); when it later resolves publicly, its tunnel
  route is removed instead of staying pinned until disconnect. Names that
  resolve to VPN-prefix addresses (e.g. `10.*`) are logged to
  `allowroute.log`.
- "Clear DNS" in the hamburger menu drops every DNS-derived tunnel route and
  flushes the OS resolver cache, for the case where a public domain was
  pinned behind the VPN by a stale answer.
- "Log Out" — a button below Disconnect while connected, and in the
  hamburger menu — ends the session entirely: it cancels any in-flight
  connect, drops the tunnel, and forgets the cached authentication so the
  next connect is a full login. Disconnect still only cuts the VPN and
  keeps a still-valid session for reuse. It replaces the old "Clear
  Credentials" menu item.
  
### Changed

- The external-browser auth wrapper (`gp-auth-browser`) is now shipped by
  the package and installed to `~/.local/bin`. It keeps opening the SSO
  flow in a dedicated window on its own persistent profile
  (`~/.cache/gp-auth-browser`), separate from the everyday browser so the
  VPN account never collides with it; `NP_AUTH_BROWSER_SHARED=1` switches
  to the everyday profile and `NP_AUTH_BROWSER_PROFILE` overrides the
  profile location.

### Fixed

- The tray "Toggle Window" action now actually raises the window above the
  others instead of leaving it buried behind them, and the main window
  appears in the dock/taskbar again so it can be restored without
  minimizing every other window.
- The login window no longer shows a generic placeholder icon: its titlebar
  icon is set to `nusaprotect`, and `StartupWMClass` entries map both the
  main window (`npgui`) and the login window (`npauth`) to the NusaProtect
  icon in the dock/taskbar.
- Manual disconnect → connect no longer runs a full login when the portal
  session is still valid. The cached authentication artifacts are reused to
  fetch portal config and authenticate to the gateway; a rejected session
  falls back to the normal SAML/password flow. "Log Out" drops the cached
  session.
- `allowroute` now watches the tunnel device: if `tun0` disappears without
  vpnc-script stopping the daemon (an orphaned disconnect), the DNS redirect
  and derived routes are torn down instead of blackholing name resolution
  until a network reset.
- Removing a tracked name no longer deletes a static route it happened to
  share (the tunnel-DNS host route), and signal-interrupted sockets no
  longer spam `Interrupted system call` errors.

## [0.1.1] - 2026-11-02

### Changed

- Publish two amd64 packages. `nusaprotect_<version>_amd64.deb` is built on the latest GitHub Ubuntu runner and served from the apt suite `stable`. `nusaprotect_<version>_ubuntu22_amd64.deb` is built on Ubuntu 22.04 and served from the suite `jammy`. `install.sh` selects the suite from the system glibc.

## [0.1.0] - 2026-10-30

### Added

- Route only non-public destinations through the VPN. System DNS is asked
  first; a name that does not resolve to a public address is resolved again
  through the tunnel, and `10.0.0.0/8` is routed there. The prefix list is
  configurable in Settings.
- Keep the tunnel client in `router/` and stop it from installing a default
  route. `allowroute` applies the DNS failover and the prefix routes.
- Publish one Debian package that contains the GUI and the VPN backend.
  Pushing a new `VERSION` to `main` tags `v<VERSION>` and updates the apt
  repository used by `apt upgrade`.
- Install that package with `./install.sh`, or build and install this tree
  with `make install`.

### Changed

- Project license unified to GPL-3.0-or-later (`LICENSE`, `src-tauri/`,
  `package.json`) to match the GPL-3.0 router subtree it links into one
  binary; the previous GPL-2.0 declaration was incompatible. The `.deb`
  now ships `/usr/share/doc/nusaprotect/copyright` with upstream and
  vendored-library attribution.
- Build NusaProtect against the local `router` crates instead of the
  upstream GlobalProtect-openconnect git repository.

### Fixed

- DNS no longer dies when resolv.conf names a resolver that only exists
  behind the tunnel (the tunnel's own address or injected corporate DNS).
  `allowroute` filters `nameserver` entries by `ip route get` output and
  skips any that resolve through the tun device or the tun's own addresses.
- One unreachable system resolver no longer stalls every DNS client. UDP
  queries are handled on bounded worker threads (64 in flight) like the TCP
  path; excess datagrams are dropped so clients retry instead of queueing.
- `allowroute` logs the effective system resolver list at startup, warns once
  per ignored nameserver, and notices when in-flight queries hit the cap.
- The single-instance guard in `npclient` matched `gpclient` in the running
  process path, so the installed `npclient` binary never counted and a second
  instance could always start. It now accepts either name.
- `npservice --version` and `npauth --version` report the installed binary
  names instead of the upstream `gpservice`/`gpauth` crate names.
- Settings About page shows the product version from `VERSION` and the license labels reflect GPL-3.0-or-later.
- Make the .deb installable across Debian and Ubuntu releases. `policykit-1`
  no longer exists on Debian trixie and newer Ubuntu; the package now
  accepts `polkitd`/`pkexec` as alternatives, and the `*t64` library names
  used by the Ubuntu 24.04 time_t transition are listed as alternatives for
  `libgtk-3-0`, `libssl3`, `libgnutls30`, `libnettle8`, and `libhogweed6`.
- Sessions no longer die at the gateway's `<timeout>` mark (~60 minutes on
  Prisma Access) when a longer `<lifetime>` is advertised (~8h). openconnect
  used to rekey the tunnel at `timeout - 60s`, which fails on
  SAML-authenticated sessions whose auth cookie has already expired and
  ends the whole session early; the rekey is now scheduled at `lifetime`
  like the PAN client does.
  


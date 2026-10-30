# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

`VERSION` is the NusaProtect package version. The GlobalProtect client
version in `src-tauri/Cargo.toml` stays aligned with the portal client
version and is not this package version.

## [Unreleased]

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
  


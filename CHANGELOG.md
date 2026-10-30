# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

`VERSION` is the NusaProtect package version. The GlobalProtect client
version in `src-tauri/Cargo.toml` stays aligned with the portal client
version and is not this package version.

## [Unreleased]

## [0.1.1] - 2026-10-30

### Fixed

- Make the .deb installable across Debian and Ubuntu releases. `policykit-1`
  no longer exists on Debian trixie and newer Ubuntu; the package now
  accepts `polkitd`/`pkexec` as alternatives, and the `*t64` library names
  used by the Ubuntu 24.04 time_t transition are listed as alternatives for
  `libgtk-3-0`, `libssl3`, `libgnutls30`, `libnettle8`, and `libhogweed6`.

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

- Build NusaProtect against the local `router` crates instead of the
  upstream GlobalProtect-openconnect git repository.

# NusaProtect
An open-source graphical front-end for the [GlobalProtect-openconnect](https://github.com/yuezk/GlobalProtect-openconnect) CLI tools, supporting password and SSO (SAML) authentication.

> **Disclaimer:** This is an unofficial third-party client and is not affiliated with or endorsed by Palo Alto Networks. GlobalProtect is a registered trademark of Palo Alto Networks.

## About

NusaProtect is a custom GUI for the `npclient` / `npservice` daemon stack. The daemon stack is built from [`router/`](router/) in this repository, which is based on [GlobalProtect-openconnect](https://github.com/yuezk/GlobalProtect-openconnect). Installed binaries use the `np*` prefix so they do not clash with the upstream `gp*` package. The GUI is built with [Tauri](https://tauri.app) (Rust + React).

NusaProtect is forked from [GP OpenUI](https://github.com/MagiShira/globalprotect-openconnect-openui) by Elane Faisal-Sage.

### Why?

The upstream `gpgui` binary is proprietary-- you can't study, modify, or share it. For a VPN client handling your credentials and network traffic, that matters. NusaProtect is [free software](https://www.fsf.org/about/what-is-free-software).

## Features

- **Password authentication** — standard username/password login
- **SSO/SAML authentication** — embedded WebView or external browser
- **Client certificate authentication** — PKCS#8 (`.pem`) and PKCS#12 (`.p12`/`.pfx`)
- **Cookie reuse** — stay logged in across SAML sessions
- **OS spoofing** — present as Linux, Windows, or macOS to the portal
- **HIP report submission** — with optional custom script path
- **Tunnel options** — disable IPv6, disable DTLS, custom MTU, VPNC script, reconnect timeout
- **Split routing** — works when **VPNC Script** in Settings is the bundled `/usr/libexec/npclient/vpnc-script` (or left empty so the GUI picks it). With the bundled script, no default route is installed. Traffic to the **VPN prefixes** in Settings (`10.0.0.0/8` by default) goes through the tunnel. System DNS is asked first; a name that fails or resolves only to non-public addresses is resolved again through the tunnel DNS, and the addresses it returns are routed through the tunnel. `allowroute` listens on `127.0.0.1:15353` so it does not clash with avahi on port 5353.
- **Theme support** — light, dark, and system-follow modes
- **Settings window** — persistent per-user settings stored locally
- **Wayland and X11** — native Wayland support, X11 fallback

## Roadmap

- [x] **Manual gateway selection** — interactive `choose-gateway` picker when the portal returns multiple gateways, a "Switch gateway" dropdown while connected, and a remembered gateway
- [x] **System tray integration** — minimize to tray, tray icon menu
- [ ] **Auto-start on login** — launch with the system and connect automatically
- [x] **Resume on wake** — npservice emits a `resume-connection` event and the app re-authenticates automatically when the "Resume on Wake Up" setting is enabled (also triggered on `reconnectRequired` VPN states)

## Installation

### Debian and Ubuntu

Run the install script as a regular user (it uses `sudo` internally where root is required):

```bash
./install.sh
```

The script adds the signed NusaProtect apt repository at `https://jahrulnr.github.io/NusaProtect` and installs the `nusaprotect` package, which contains the GUI and the VPN backend. If that repository is not published yet, it installs `nusaprotect_<version>_amd64.deb` from the newest `v*` release on [github.com/jahrulnr/NusaProtect](https://github.com/jahrulnr/NusaProtect) instead. In that case it does not add the apt source, so run `./install.sh` again after the repository is published.

Once the apt source is configured, later versions install with `sudo apt update && sudo apt upgrade`. To pin the GitHub release used by the fallback, set `NUSAPROTECT_RELEASE=v<version>`. A private checkout needs `gh auth login`.

Prebuilt packages are amd64 only.

> **Note:** NusaProtect installs `/usr/bin/npclient`, `npservice`, `npauth`, and `npgui`, so it can sit beside upstream `globalprotect-openconnect` without file conflicts.

### Other distributions

`install.sh` supports only Debian and Ubuntu. On other distributions, build and install from this tree with `make install`. See [Build from source](#build-from-source).

## Usage

After installation, launch NusaProtect through your application launcher or via:

```bash
npclient launch-gui
```

### Legacy TLS Configurations

Some GlobalProtect installations use older TLS configurations (e.g. deprecated ciphers or older protocol versions) that are rejected by the system OpenSSL by default. If the GUI fails to connect with a TLS handshake error, use the `--fix-openssl` flag:

```bash
npclient --fix-openssl launch-gui
```

## Build from source

`make install` builds the backend in `router/` and the GUI in `src-tauri/`, then installs both into `/usr` with `sudo`. Install the build dependencies first. On Debian and Ubuntu:

```bash
sudo apt-get install -y build-essential pkg-config autoconf automake autopoint libtool \
  libssl-dev zlib1g-dev liblz4-dev libxml2-dev libgnutls28-dev nettle-dev libp11-kit-dev libgmp-dev \
  libgtk-3-dev libwebkit2gtk-4.1-dev libjavascriptcoregtk-4.1-dev librsvg2-dev libsoup-3.0-dev patchelf
```

The OpenConnect build in `router/` needs the pkg-config modules `libxml-2.0`, `zlib`, `liblz4`, `gnutls`, `p11-kit-1`, `hogweed`, `nettle`, and `gmp` on Linux. The `-dev` packages above provide them on Ubuntu 24.04.

You also need Rust (`router/rust-toolchain.toml` pins 1.91), pnpm (the version in the `packageManager` field of `package.json`), and Node.js 22 (the version CI uses).

| Command | What it does |
|---|---|
| `make backend` | Build backend crates (`gpclient`, `gpservice`, `gpauth`, `allowroute`) in release mode |
| `make frontend` | Run `pnpm install --frozen-lockfile`, then build the GUI binary without an installer |
| `make all` | Run `make backend`, then `make frontend` |
| `make install` | Run `make all`, then install into `/usr` as `npclient` / `npservice` / `npauth` / `npgui`, libexec, NM hooks, icons, and polkit policy |
| `make check-version` | Check that `VERSION` is `x.y.z` and that `CHANGELOG.md` has a section for it |
| `make backend-dist` | Pack the backend binaries into `.build/nusaprotect-backend-linux-<arch>.tar.gz` |

`make install` copies files without a package manager. Upstream "GP Connect" (`gpgui.desktop`) can remain in the menu if that package is installed; NusaProtect uses `nusaprotect.desktop`.

## Releases

- `VERSION` is the package version. The `version` in `src-tauri/Cargo.toml` tracks the upstream GlobalProtect client version and is not the package version.
- Each version needs a `## [<version>]` section in [CHANGELOG.md](CHANGELOG.md). `make check-version` checks it.
- A push to `main` builds the backend and GUI. If tag `v<VERSION>` does not exist yet, CI publishes the `.deb`, the apt repository, and a GitHub release. To publish again, bump `VERSION` and add its changelog section.

## Requirements

- Linux (X11 or Wayland)
- Debian or Ubuntu for `./install.sh`. Other distributions use `make install`, which needs the [build dependencies](#build-from-source).
- Internet access during the build for Cargo and pnpm dependencies. The router workspace also patches `tao` from GitHub.

## License

NusaProtect (`src/`, `src-tauri/`) is GPL-2.0; see [LICENSE](LICENSE). `router/` is GPL-3.0; see [router/LICENSE](router/LICENSE). Some crates in `router/` are MIT, so check each crate's `Cargo.toml`.

#!/usr/bin/env bash
# install.sh — Install the NusaProtect .deb from GitHub.
#
# The package contains the GUI and the VPN backend. Debian and Ubuntu can
# install it with apt. Other distributions should use `make install`, which
# builds this tree and does not download anything.
#
# CI publishes one tag, v<VERSION>, from the top-level VERSION file. A new tag
# is created only when that package version is not already published. A private
# repository needs `gh auth login`.

set -euo pipefail

NUSAPROTECT_REPO="${NUSAPROTECT_REPO:-jahrulnr/NusaProtect}"
NUSAPROTECT_APT_URL="${NUSAPROTECT_APT_URL:-https://jahrulnr.github.io/NusaProtect}"
# Empty means the newest v* package release. Set this to pin a tag.
NUSAPROTECT_RELEASE="${NUSAPROTECT_RELEASE:-}"

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------
info()    { printf '\033[0;32m[INFO]\033[0m  %s\n' "$*"; }
error()   { printf '\033[0;31m[ERROR]\033[0m %s\n' "$*" >&2; }
die()     { error "$*"; exit 1; }

have() { command -v "$1" &>/dev/null; }

deb_arch() {
  case "$(uname -m)" in
    x86_64) printf 'amd64\n' ;;
    aarch64|arm64) printf 'arm64\n' ;;
    *) die "No .deb is published for $(uname -m). Run 'make install' to build from this tree." ;;
  esac
}

resolve_release_tag() {
  local tag
  if [[ -n "$NUSAPROTECT_RELEASE" ]]; then
    printf '%s\n' "$NUSAPROTECT_RELEASE"
    return
  fi
  if have gh && gh auth status >/dev/null 2>&1; then
    tag="$(gh release list --repo "$NUSAPROTECT_REPO" --limit 100 --json tagName \
      --jq '[.[] | select(.tagName | test("^v[0-9]"))][0].tagName')"
  else
    tag="$(curl -fsSL "https://api.github.com/repos/${NUSAPROTECT_REPO}/releases?per_page=100" \
      | python3 -c 'import json,sys,re
rels=json.load(sys.stdin)
for rel in rels:
    name=rel.get("tag_name","")
    if re.match(r"v[0-9]", name):
        print(name)
        break')"
  fi
  if [[ -z "$tag" || "$tag" == "null" ]]; then
    die "No v* release found on ${NUSAPROTECT_REPO}. Set NUSAPROTECT_RELEASE to a tag, or run 'make install' to build from this tree."
  fi
  printf '%s\n' "$tag"
}

apt_repo_ready() {
  curl -fsI --retry 2 "$NUSAPROTECT_APT_URL/dists/stable/InRelease" >/dev/null 2>&1
}

configure_apt_source() {
  sudo install -d /usr/share/keyrings
  curl -fsSL "$NUSAPROTECT_APT_URL/nusaprotect.gpg" | sudo tee /usr/share/keyrings/nusaprotect.gpg >/dev/null
  printf 'deb [signed-by=/usr/share/keyrings/nusaprotect.gpg] %s stable main\n' "$NUSAPROTECT_APT_URL" \
    | sudo tee /etc/apt/sources.list.d/nusaprotect.list >/dev/null
}

install_deb() {
  local arch asset url tmp tag
  if ! have apt-get; then
    die "The GitHub package is a .deb for Debian and Ubuntu. On this system, run 'make install'."
  fi
  arch="$(deb_arch)"
  tag="$(resolve_release_tag)"
  asset="nusaprotect_${tag#v}_${arch}.deb"
  url="https://github.com/${NUSAPROTECT_REPO}/releases/download/${tag}/${asset}"
  tmp="$(mktemp -d)"

  info "Downloading ${asset} from ${NUSAPROTECT_REPO} release ${tag}..."
  if have gh && gh auth status >/dev/null 2>&1; then
    gh release download "$tag" --repo "$NUSAPROTECT_REPO" --pattern "$asset" --dir "$tmp" --clobber \
      || die "Could not download ${asset} from ${NUSAPROTECT_REPO} (${tag})."
  else
    curl -fL "$url" -o "${tmp}/${asset}" \
      || die "Could not download ${url}. If the repository is still private, run 'gh auth login' and retry."
  fi

  info "Installing ${asset}..."
  sudo apt-get update -q
  sudo apt-get install -y "${tmp}/${asset}"
  rm -rf "$tmp"
  info "Installed ${asset}."
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------
main() {
  info "=== NusaProtect installer ==="
  echo
  if ! have apt-get; then
    die "The GitHub package is a .deb for Debian and Ubuntu. On this system, run 'make install'."
  fi
  if apt_repo_ready; then
    info "Configuring the apt source at ${NUSAPROTECT_APT_URL}..."
    configure_apt_source
    sudo apt-get update -q
    sudo apt-get install -y nusaprotect
    info "Sumber apt terpasang. Versi berikutnya masuk lewat: sudo apt update && sudo apt upgrade"
  else
    install_deb
    info "Paket terpasang dari rilis GitHub. apt update melihat versi baru setelah repositori dan GitHub Pages publik."
  fi
  echo
  info "=== Done! ==="
  info "Launch NusaProtect from the application menu or run: npclient launch-gui"
}

main "$@"

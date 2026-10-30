#!/bin/sh
# Pack the GUI and the VPN backend into one .deb.
# Usage: scripts/package-deb.sh --version 3.2.0 --backend-dir DIR --gui FILE --output FILE
set -eu

version=
backend_dir=
gui=
output=
arch=amd64

while [ $# -gt 0 ]; do
  case "$1" in
    --version) version=$2; shift 2 ;;
    --backend-dir) backend_dir=$2; shift 2 ;;
    --gui) gui=$2; shift 2 ;;
    --output) output=$2; shift 2 ;;
    --arch) arch=$2; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

if [ -z "$version" ] || [ -z "$backend_dir" ] || [ -z "$gui" ] || [ -z "$output" ]; then
  echo "version, backend directory, gui binary, and output path are required" >&2
  exit 2
fi

root=$(CDPATH= cd "$(dirname "$0")/.." && pwd)
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT

pkg="$stage/nusaprotect"
install -d \
  "$pkg/DEBIAN" \
  "$pkg/usr/bin" \
  "$pkg/usr/libexec/gpclient" \
  "$pkg/usr/share/applications" \
  "$pkg/usr/share/polkit-1/actions" \
  "$pkg/usr/share/icons/hicolor/scalable/apps" \
  "$pkg/usr/share/icons/hicolor/32x32/apps" \
  "$pkg/usr/share/icons/hicolor/64x64/apps" \
  "$pkg/usr/share/icons/hicolor/128x128/apps" \
  "$pkg/usr/share/icons/hicolor/256x256/apps" \
  "$pkg/usr/share/icons/hicolor/512x512/apps"

install -m 755 "$backend_dir/bin/gpclient" "$pkg/usr/bin/gpclient"
install -m 755 "$backend_dir/bin/gpservice" "$pkg/usr/bin/gpservice"
install -m 755 "$backend_dir/bin/gpauth" "$pkg/usr/bin/gpauth"
install -m 755 "$gui" "$pkg/usr/bin/gpgui"
install -m 755 "$backend_dir/libexec/gpclient/allowroute" "$pkg/usr/libexec/gpclient/allowroute"
install -m 755 "$backend_dir/libexec/gpclient/vpnc-script" "$pkg/usr/libexec/gpclient/vpnc-script"
install -m 755 "$backend_dir/libexec/gpclient/gp-vpnc-script-installer" "$pkg/usr/libexec/gpclient/gp-vpnc-script-installer"
install -m 755 "$backend_dir/libexec/gpclient/gp-hip-script-installer" "$pkg/usr/libexec/gpclient/gp-hip-script-installer"
install -m 644 "$root/router/packaging/files/usr/share/polkit-1/actions/com.yuezk.gpgui.policy" \
  "$pkg/usr/share/polkit-1/actions/com.yuezk.gpgui.policy"
install -m 644 "$root/packaging/nusaprotect.desktop" "$pkg/usr/share/applications/nusaprotect.desktop"
install -m 644 "$root/src-tauri/icons/icon.svg" "$pkg/usr/share/icons/hicolor/scalable/apps/nusaprotect.svg"
install -m 644 "$root/src-tauri/icons/32x32.png" "$pkg/usr/share/icons/hicolor/32x32/apps/nusaprotect.png"
install -m 644 "$root/src-tauri/icons/64x64.png" "$pkg/usr/share/icons/hicolor/64x64/apps/nusaprotect.png"
install -m 644 "$root/src-tauri/icons/128x128.png" "$pkg/usr/share/icons/hicolor/128x128/apps/nusaprotect.png"
install -m 644 "$root/src-tauri/icons/128x128@2x.png" "$pkg/usr/share/icons/hicolor/256x256/apps/nusaprotect.png"
install -m 644 "$root/src-tauri/icons/icon.png" "$pkg/usr/share/icons/hicolor/512x512/apps/nusaprotect.png"

cat > "$pkg/DEBIAN/control" <<EOF
Package: nusaprotect
Version: $version
Section: net
Priority: optional
Architecture: $arch
Maintainer: NusaProtect <https://github.com/jahrulnr/NusaProtect>
Depends: libgtk-3-0, libwebkit2gtk-4.1-0, libsoup-3.0-0, libssl3, ca-certificates, xdg-utils, libayatana-appindicator3-1
Description: NusaProtect GlobalProtect client
 GUI and tunnel programs in one package. Public addresses stay on the normal
 network. 10.0.0.0/8 and other non-public answers go through the tunnel.
EOF

cat > "$pkg/DEBIAN/postinst" <<'EOF'
#!/bin/sh
set -e
if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database /usr/share/applications || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -f -t /usr/share/icons/hicolor || true
fi
EOF
chmod 755 "$pkg/DEBIAN/postinst"

mkdir -p "$(dirname "$output")"
dpkg-deb --root-owner-group --build "$pkg" "$output" >/dev/null
echo "$output"

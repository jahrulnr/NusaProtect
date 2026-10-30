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
  "$pkg/usr/libexec/npclient" \
  "$pkg/usr/share/applications" \
  "$pkg/usr/share/polkit-1/actions" \
  "$pkg/usr/share/icons/hicolor/scalable/apps" \
  "$pkg/usr/share/icons/hicolor/32x32/apps" \
  "$pkg/usr/share/icons/hicolor/64x64/apps" \
  "$pkg/usr/share/icons/hicolor/128x128/apps" \
  "$pkg/usr/share/icons/hicolor/256x256/apps" \
  "$pkg/usr/share/icons/hicolor/512x512/apps" \
  "$pkg/usr/lib/NetworkManager/dispatcher.d/pre-down.d"

install -m 755 "$backend_dir/bin/npclient" "$pkg/usr/bin/npclient"
install -m 755 "$backend_dir/bin/npservice" "$pkg/usr/bin/npservice"
install -m 755 "$backend_dir/bin/npauth" "$pkg/usr/bin/npauth"
install -m 755 "$gui" "$pkg/usr/bin/npgui"
install -m 755 "$backend_dir/libexec/npclient/allowroute" "$pkg/usr/libexec/npclient/allowroute"
install -m 755 "$backend_dir/libexec/npclient/vpnc-script" "$pkg/usr/libexec/npclient/vpnc-script"
install -m 755 "$backend_dir/libexec/npclient/np-vpnc-script-installer" "$pkg/usr/libexec/npclient/np-vpnc-script-installer"
install -m 755 "$backend_dir/libexec/npclient/np-hip-script-installer" "$pkg/usr/libexec/npclient/np-hip-script-installer"
install -m 644 "$root/router/packaging/files/usr/share/polkit-1/actions/io.github.jahrulnr.nusaprotect.policy" \
  "$pkg/usr/share/polkit-1/actions/io.github.jahrulnr.nusaprotect.policy"
install -m 755 "$root/router/packaging/files/usr/lib/NetworkManager/dispatcher.d/npclient-nm-hook" \
  "$pkg/usr/lib/NetworkManager/dispatcher.d/npclient-nm-hook"
install -m 755 "$root/router/packaging/files/usr/lib/NetworkManager/dispatcher.d/pre-down.d/npclient.down" \
  "$pkg/usr/lib/NetworkManager/dispatcher.d/pre-down.d/npclient.down"
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
Depends: libgtk-3-0 | libgtk-3-0t64, libwebkit2gtk-4.1-0, libsoup-3.0-0, libssl3 | libssl3t64, ca-certificates, xdg-utils, libayatana-appindicator3-1, policykit-1 | polkitd, pkexec | policykit-1, nftables, libxml2, libgnutls30 | libgnutls30t64, libnettle8 | libnettle8t64, libhogweed6 | libhogweed6t64, libp11-kit0, libgmp10, liblz4-1
Description: NusaProtect GlobalProtect client
 GUI and tunnel programs in one package. Public addresses stay on the normal
 network. 10.0.0.0/8 and other non-public answers go through the tunnel.
 Binaries use the np* prefix so this package can sit beside upstream
 globalprotect-openconnect.
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

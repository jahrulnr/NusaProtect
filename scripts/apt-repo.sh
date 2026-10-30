#!/bin/sh
# Add one .deb to an apt repository directory and sign dists/stable.
# Usage: scripts/apt-repo.sh --deb FILE --repo DIR --key FINGERPRINT
set -eu

deb=
repo=
key=

while [ $# -gt 0 ]; do
  case "$1" in
    --deb) deb=$2; shift 2 ;;
    --repo) repo=$2; shift 2 ;;
    --key) key=$2; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

if [ -z "$deb" ] || [ -z "$repo" ] || [ -z "$key" ]; then
  echo "deb, repository directory, and signing key are required" >&2
  exit 2
fi

root=$(CDPATH= cd "$(dirname "$0")/.." && pwd)
name=$(basename "$deb")
install -d "$repo/pool/main/n/nusaprotect" "$repo/dists/stable/main/binary-amd64"
install -m 644 "$deb" "$repo/pool/main/n/nusaprotect/$name"
install -m 644 "$root/packaging/apt/nusaprotect.gpg" "$repo/nusaprotect.gpg"

(
  cd "$repo"
  dpkg-scanpackages pool /dev/null > dists/stable/main/binary-amd64/Packages
  gzip -9 -c dists/stable/main/binary-amd64/Packages > dists/stable/main/binary-amd64/Packages.gz
)

release="$repo/dists/stable/Release"
{
  echo "Origin: NusaProtect"
  echo "Label: NusaProtect"
  echo "Suite: stable"
  echo "Codename: stable"
  echo "Architectures: amd64"
  echo "Components: main"
  echo "Date: $(date -Ru)"
  echo "SHA256:"
  for file in main/binary-amd64/Packages main/binary-amd64/Packages.gz; do
    hash=$(sha256sum "$repo/dists/stable/$file" | awk '{print $1}')
    size=$(wc -c < "$repo/dists/stable/$file")
    printf ' %s %8d %s\n' "$hash" "$size" "$file"
  done
} > "$release"

gpg --batch --yes --local-user "$key" --detach-sign --armor -o "$repo/dists/stable/Release.gpg" "$release"
gpg --batch --yes --local-user "$key" --clearsign -o "$repo/dists/stable/InRelease" "$release"
echo "$repo"

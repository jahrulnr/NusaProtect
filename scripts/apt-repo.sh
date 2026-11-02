#!/bin/sh
# Add one .deb to an apt repository directory and sign dists/<suite>.
# Usage: scripts/apt-repo.sh --deb FILE --repo DIR --key FINGERPRINT [--suite stable|jammy]
#
# stable scans pool/ (the historical layout). jammy scans pool-jammy/ so the
# Ubuntu 22.04 build can share the package name and version with the latest
# build without landing in the same Packages file.
set -eu

deb=
repo=
key=
suite=stable

while [ $# -gt 0 ]; do
  case "$1" in
    --deb) deb=$2; shift 2 ;;
    --repo) repo=$2; shift 2 ;;
    --key) key=$2; shift 2 ;;
    --suite) suite=$2; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

if [ -z "$deb" ] || [ -z "$repo" ] || [ -z "$key" ]; then
  echo "deb, repository directory, and signing key are required" >&2
  exit 2
fi

case "$suite" in
  stable) pool=pool ;;
  jammy) pool=pool-jammy ;;
  *) echo "unknown suite: $suite" >&2; exit 2 ;;
esac

root=$(CDPATH= cd "$(dirname "$0")/.." && pwd)
name=$(basename "$deb")
install -d "$repo/$pool/main/n/nusaprotect" "$repo/dists/$suite/main/binary-amd64"
install -m 644 "$deb" "$repo/$pool/main/n/nusaprotect/$name"
install -m 644 "$root/packaging/apt/nusaprotect.gpg" "$repo/nusaprotect.gpg"

(
  cd "$repo"
  dpkg-scanpackages "$pool" /dev/null > "dists/$suite/main/binary-amd64/Packages"
  gzip -9 -c "dists/$suite/main/binary-amd64/Packages" > "dists/$suite/main/binary-amd64/Packages.gz"
)

release="$repo/dists/$suite/Release"
{
  echo "Origin: NusaProtect"
  echo "Label: NusaProtect"
  echo "Suite: $suite"
  echo "Codename: $suite"
  echo "Architectures: amd64"
  echo "Components: main"
  echo "Date: $(date -Ru)"
  echo "SHA256:"
  for file in main/binary-amd64/Packages main/binary-amd64/Packages.gz; do
    hash=$(sha256sum "$repo/dists/$suite/$file" | awk '{print $1}')
    size=$(wc -c < "$repo/dists/$suite/$file")
    printf ' %s %8d %s\n' "$hash" "$size" "$file"
  done
} > "$release"

gpg --batch --yes --local-user "$key" --detach-sign --armor -o "$repo/dists/$suite/Release.gpg" "$release"
gpg --batch --yes --local-user "$key" --clearsign -o "$repo/dists/$suite/InRelease" "$release"
echo "$repo"

#!/bin/sh
set -eu

repo="myersguo/cdisk"
asset="CDisk-macos-arm64-unsigned.zip"
checksum_asset="SHA256SUMS"
version="${CDISK_VERSION:-}"

if [ "$(uname -s)" != "Darwin" ] || [ "$(uname -m)" != "arm64" ]; then
  echo "CDisk currently supports Apple Silicon Macs only." >&2
  exit 1
fi

for command in curl ditto shasum codesign plutil; do
  if ! command -v "$command" >/dev/null 2>&1; then
    echo "Required command not found: $command" >&2
    exit 1
  fi
done

if pgrep -x cdisk >/dev/null 2>&1; then
  echo "Quit CDisk before installing or upgrading it." >&2
  exit 1
fi

if [ -n "$version" ]; then
  case "$version" in
    v*) tag="$version" ;;
    *) tag="v$version" ;;
  esac
  base_url="https://github.com/$repo/releases/download/$tag"
else
  base_url="https://github.com/$repo/releases/latest/download"
fi

temporary="$(mktemp -d "${TMPDIR:-/tmp}/cdisk-install.XXXXXX")"
trap 'rm -rf "$temporary"' EXIT HUP INT TERM

curl --proto '=https' --tlsv1.2 --fail --location --retry 3 \
  "$base_url/$asset" -o "$temporary/$asset"
curl --proto '=https' --tlsv1.2 --fail --location --retry 3 \
  "$base_url/$checksum_asset" -o "$temporary/$checksum_asset"

expected="$(awk -v asset="$asset" '$2 == asset || $2 == "*" asset { print $1; exit }' "$temporary/$checksum_asset")"
actual="$(shasum -a 256 "$temporary/$asset" | awk '{print $1}')"
if [ -z "$expected" ] || [ "$actual" != "$expected" ]; then
  echo "CDisk archive checksum verification failed." >&2
  exit 1
fi

ditto -x -k "$temporary/$asset" "$temporary/unpacked"
app="$temporary/unpacked/CDisk.app"
if [ ! -d "$app" ]; then
  echo "The downloaded archive does not contain CDisk.app." >&2
  exit 1
fi
if [ "$(plutil -extract CFBundleIdentifier raw "$app/Contents/Info.plist")" != "com.myersguo.cdisk" ]; then
  echo "The downloaded application has an unexpected bundle identifier." >&2
  exit 1
fi
codesign --verify --deep --strict "$app"

install_dir="${CDISK_INSTALL_DIR:-}"
if [ -z "$install_dir" ]; then
  if [ -w /Applications ]; then
    install_dir="/Applications"
  else
    install_dir="$HOME/Applications"
  fi
fi
mkdir -p "$install_dir"

destination="$install_dir/CDisk.app"
staged="$install_dir/.CDisk.app.new.$$"
backup="$install_dir/.CDisk.app.backup.$$"
ditto "$app" "$staged"
if [ -e "$destination" ]; then
  mv "$destination" "$backup"
fi
if ! mv "$staged" "$destination"; then
  [ ! -e "$backup" ] || mv "$backup" "$destination"
  exit 1
fi
rm -rf "$backup"

echo "Installed CDisk to $destination"
echo "This build is ad-hoc signed and not notarized. If macOS blocks it, use Finder's Open action to review and launch it."

#!/usr/bin/env bash
# Builds DentalCraft.app (universal: Apple Silicon and Intel) and the release archive
# dentalcraft_<V>_macos_universal.zip. Runs on a Mac only: it needs lipo, codesign, iconutil,
# plutil and ditto from Xcode's command line tools, and the Rust toolchain of the repository.
#
#   bash packaging/macos/bundle.sh <version>            # 0.2.0-alpha.12, no leading v
#   VERSION=0.2.0-alpha.12 NO_BUILD=1 bash packaging/macos/bundle.sh
#
# Run it from the root of dentalcraft-app (a checkout of the release tag, outside the
# superproject: its .cargo/config.toml holds the local [patch], sccache and the mold linker,
# which a release build never uses). It builds `dentalcraft` and `dentalcraft-cli` for both
# targets (NO_BUILD=1 reuses target/<triple>/release), joins them with lipo, and writes:
#
#   target/macos/DentalCraft.app/Contents/MacOS/{dentalcraft,dentalcraft-cli}   universal
#   target/macos/DentalCraft.app/Contents/Info.plist, Resources/{DentalCraft.icns, install-kind,
#     LICENSE.md, THIRD-PARTY-NOTICES.md, README.md, LICENSES/, it.lproj/InfoPlist.strings}
#   dist/dentalcraft_<V>_macos_universal.zip                            the release asset
#
# `install-kind` (macos-app) is in Resources, not next to the programs: files in MacOS/ would
# have to be code. The in-app updater (dc-update) reads it there.
# The signature is ad-hoc (`codesign --sign -`): no Developer ID, no notarization. Apple
# Silicon refuses unsigned code, and Gatekeeper asks the user to allow the app once
# (System Settings > Privacy & Security > Open Anyway).
set -euo pipefail

cd "$(dirname "$0")/../.."
VERSION="${1:-${VERSION:-}}"
VERSION="${VERSION#v}"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] || {
  echo "usage: bash packaging/macos/bundle.sh <version>  (for example 0.2.0-alpha.12)" >&2
  exit 1
}
BUNDLE_VERSION="${VERSION%%-*}"
TARGETS=(aarch64-apple-darwin x86_64-apple-darwin)
BINS=(dentalcraft dentalcraft-cli)
PKG=packaging/macos
OUT="${OUT:-dist}"
APP="${APP_DIR:-target/macos}/DentalCraft.app"
ZIP="$OUT/dentalcraft_${VERSION}_macos_universal.zip"
# macOS 11 is the oldest system the bundle claims (Info.plist LSMinimumSystemVersion)
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-11.0}"

if [[ "${NO_BUILD:-0}" != 1 ]]; then
  for t in "${TARGETS[@]}"; do
    rustup target add "$t"
    cargo build --release --locked -p dc-app -p dc-cli --target "$t"
  done
fi

rm -rf "$APP" "$ZIP"
mkdir -p "$OUT" "$APP/Contents/MacOS" "$APP/Contents/Resources/LICENSES" "$APP/Contents/Resources/it.lproj"

# one universal binary per program
for b in "${BINS[@]}"; do
  lipo -create \
    "target/${TARGETS[0]}/release/$b" \
    "target/${TARGETS[1]}/release/$b" \
    -output "$APP/Contents/MacOS/$b"
  lipo -info "$APP/Contents/MacOS/$b"
done

sed -e "s/@VERSION@/$VERSION/g" -e "s/@BUNDLE_VERSION@/$BUNDLE_VERSION/g" \
  "$PKG/Info.plist.in" > "$APP/Contents/Info.plist"
plutil -lint "$APP/Contents/Info.plist"
plutil -lint "$PKG/entitlements.plist"
cp "$PKG/InfoPlist.it.strings" "$APP/Contents/Resources/it.lproj/InfoPlist.strings"
plutil -lint "$APP/Contents/Resources/it.lproj/InfoPlist.strings"

# the icon, from the Linux hicolor PNGs (same artwork): .icns holds 16 to 512 px (+ @2x)
iconset="$(mktemp -d)/DentalCraft.iconset"
mkdir -p "$iconset"
icon() { cp "packaging/linux/icons/hicolor/$1x$1/apps/com.dentalcraft.DentalCraft.png" "$iconset/$2.png"; }
icon 16 icon_16x16
icon 32 icon_16x16@2x
icon 32 icon_32x32
icon 64 icon_32x32@2x
icon 128 icon_128x128
icon 256 icon_128x128@2x
icon 256 icon_256x256
icon 512 icon_256x256@2x
icon 512 icon_512x512
iconutil --convert icns --output "$APP/Contents/Resources/DentalCraft.icns" "$iconset"
rm -rf "$(dirname "$iconset")"

# licence, notices, README, the licences of the bundled fonts, and the updater's marker
cp LICENSE.md THIRD-PARTY-NOTICES.md README.md "$APP/Contents/Resources/"
find crates -path '*/assets/fonts/*' -type f \( -name 'OFL*' -o -name 'LICENSE*' \) | while read -r f; do
  d="$APP/Contents/Resources/LICENSES/$(basename "$(dirname "$f")")"
  mkdir -p "$d" && cp "$f" "$d/"
done
printf 'macos-app\n' > "$APP/Contents/Resources/install-kind"

# Ad-hoc signature, the programs first and the bundle (which seals Info.plist and
# Resources) last. `--deep` is left out of the signing on purpose: both programs are already
# signed, with their entitlements; it is only used to verify.
for b in "${BINS[@]}"; do
  codesign --force --sign - --entitlements "$PKG/entitlements.plist" \
    --identifier "com.dentalcraft.DentalCraft${b#dentalcraft}" "$APP/Contents/MacOS/$b"
done
codesign --force --sign - --entitlements "$PKG/entitlements.plist" "$APP"
codesign --verify --deep --strict --verbose=2 "$APP"
codesign -d --entitlements :- "$APP" >/dev/null

# the release asset: `ditto` keeps the permissions and the top folder; no resource forks,
# extended attributes or quarantine flag go into it
ditto -c -k --keepParent --norsrc --noextattr --noqtn "$APP" "$ZIP"
ls -l "$ZIP"
shasum -a 256 "$ZIP"

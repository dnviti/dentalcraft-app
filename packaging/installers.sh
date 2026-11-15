#!/usr/bin/env bash
# Builds the installers of a DentalCraft release from its two standalone archives,
#   dist/dentalcraft_<V>_linux_x86_64.tar.gz and dist/dentalcraft_<V>_windows_x86_64.zip,
# into installers/:
#   DentalCraft-<V>-x86_64.AppImage   dentalcraft_<V>_amd64.deb   dentalcraft-<V>-1.x86_64.rpm
#   DentalCraft-<V>.flatpak           DentalCraft-<V>-setup.exe   DentalCraft-<V>-x86_64.msi
# so every package holds exactly the binaries of the archives.
#
# The installers job of .github/workflows/binaries.yml runs it, and so does
# `scripts/release-local.sh binaries app` of the dentalcraft superproject: as root, in
# a throwaway registry.fedoraproject.org/fedora:45 container started with --privileged
# (flatpak-builder runs its build in a bubblewrap sandbox), from the root of the checkout:
#
#   TAG=vX.Y.Z[-pre.N] SOURCE_DATE_EPOCH=<tag commit time> \
#   DENTALCRAFT_GPG_KEY_FILE=<file with the armored secret release key> \
#     bash packaging/installers.sh [appimage|deb|rpm|flatpak|exe|msi]...   (default: all)
#
# The rpm and the Flatpak commit are signed with the release key, which must be the key
# published in packaging/keys/dentalcraft.gpg. Without a key the run fails, unless
# ALLOW_UNSIGNED=1 (local tests only: nothing unsigned is ever published).
# DC_PKG_CACHE: optional directory kept between runs (tool downloads, Flatpak runtime).
# Every format, its tools and its layout: packaging/README.md.
# The format scripts are sourced and read the variables set here.
# shellcheck disable=SC2034
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

APP_ID=com.dentalcraft.DentalCraft
# AppImage tooling, pinned with checksums (sha256 of the GitHub release assets).
APPIMAGETOOL_URL=https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-x86_64.AppImage
APPIMAGETOOL_SHA256=ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0
APPIMAGE_RUNTIME_URL=https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-x86_64
APPIMAGE_RUNTIME_SHA256=2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d
# Flatpak: repository of the site, remote of the runtime (the runtime itself is in the
# manifest, packaging/flatpak/com.dentalcraft.DentalCraft.yml).
FLATPAK_REPO_URL=https://dentalcraft.app/linux/flatpak/
FLATHUB_REPO_URL=https://dl.flathub.org/repo/flathub.flatpakrepo
# The tools, from the Fedora 45 repositories.
TOOLS="rpm-build rpm-sign dpkg squashfs-tools desktop-file-utils appstream flatpak flatpak-builder
git-core mingw32-nsis msitools gnupg2 binutils file unzip xz zstd"

die() { printf '##[error]%s\n' "$*" >&2; exit 1; }
warn() { printf '##[warning]%s\n' "$*" >&2; }
group() { printf '\n##[group]%s\n' "$*"; }
endgroup() { printf '##[endgroup]\n'; }

# ---------------------------------------------------------------- inputs

[ -n "${TAG:-}" ] || die "TAG is not set (vX.Y.Z or vX.Y.Z-pre.N)"
[ -n "${SOURCE_DATE_EPOCH:-}" ] || die "SOURCE_DATE_EPOCH is not set (the commit time of the tag)"
export SOURCE_DATE_EPOCH
VERSION=${TAG#v}
# Package versions: the pre-release goes after a tilde, which sorts before the release
# in dpkg and rpm (0.2.0~alpha.4 < 0.2.0). Windows wants four numbers: the pre-release
# number becomes the fourth (0.2.0-alpha.4 -> 0.2.0.4, 0.2.0 -> 0.2.0.0); MSI upgrades
# compare only the first three.
if [[ $VERSION =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)(-([0-9A-Za-z]+)(\.([0-9]+))?)?$ ]]; then
  PKG_VERSION=${VERSION/-/\~}
  WIN_VERSION=${BASH_REMATCH[1]}.${BASH_REMATCH[2]}.${BASH_REMATCH[3]}.${BASH_REMATCH[7]:-0}
  [ "${BASH_REMATCH[1]}" -le 255 ] && [ "${BASH_REMATCH[2]}" -le 255 ] && [ "${BASH_REMATCH[3]}" -le 65535 ] \
    && [ "${BASH_REMATCH[7]:-0}" -le 65535 ] || die "version $VERSION does not fit a Windows version (255.255.65535.65535)"
else
  die "tag must look like vX.Y.Z or vX.Y.Z-<word>.N, got '$TAG'"
fi
RELEASE_DATE=$(date -u -d "@$SOURCE_DATE_EPOCH" +%F)
LINUX_ARCHIVE=dist/dentalcraft_${VERSION}_linux_x86_64.tar.gz
WINDOWS_ARCHIVE=dist/dentalcraft_${VERSION}_windows_x86_64.zip
[ -f "$LINUX_ARCHIVE" ] || die "missing $LINUX_ARCHIVE"
[ -f "$WINDOWS_ARCHIVE" ] || die "missing $WINDOWS_ARCHIVE"

FORMATS=("$@")
[ ${#FORMATS[@]} -gt 0 ] || FORMATS=(appimage deb rpm flatpak exe msi)
for f in "${FORMATS[@]}"; do
  case "$f" in appimage|deb|rpm|flatpak|exe|msi) ;; *) die "unknown format $f" ;; esac
done

OUT=$PWD/installers
WORK=$(mktemp -d /tmp/dentalcraft-installers.XXXXXX)
trap 'rm -rf "$WORK"' EXIT
CACHE_DIR=${DC_PKG_CACHE:-$WORK/cache}
mkdir -p "$OUT" "$CACHE_DIR/downloads"
umask 022

group "Install the packaging tools (Fedora $(. /etc/os-release && echo "$VERSION_ID"))"
# shellcheck disable=SC2086 # TOOLS is a word list
dnf install -y -q --setopt=install_weak_deps=False $TOOLS >/dev/null
# shellcheck disable=SC2086
rpm -q $TOOLS | sort
endgroup

# A file of the cache, downloaded once and checked against its pinned sha256.
fetch() {
  local url=$1 sum=$2 file=$CACHE_DIR/downloads/${1##*/}
  if [ ! -f "$file" ] || ! printf '%s  %s\n' "$sum" "$file" | sha256sum -c --quiet - 2>/dev/null; then
    curl -fsSL --retry 3 -o "$file.part" "$url"
    printf '%s  %s\n' "$sum" "$file.part" | sha256sum -c --quiet - || die "checksum mismatch for $url"
    mv "$file.part" "$file"
  fi
  printf '%s\n' "$file"
}

# ---------------------------------------------------------------- signing key

PUBKEY_ARMORED=packaging/keys/dentalcraft.gpg
# The binary form of the public key: the apt keyring of the deb and the key embedded
# in the Flatpak bundle. The site serves the same bytes at /linux/dentalcraft.gpg.
PUBKEY_BINARY=$WORK/dentalcraft.gpg
gpg --dearmor < "$PUBKEY_ARMORED" > "$PUBKEY_BINARY"
PUBKEY_FPR=$(gpg --batch --with-colons --show-keys "$PUBKEY_ARMORED" | awk -F: '$1 == "fpr" {print $10; exit}')
GPG_FPR=
export GNUPGHOME=$WORK/gnupg
mkdir -m 700 "$GNUPGHOME"
if [ -n "${DENTALCRAFT_GPG_KEY_FILE:-}" ] && grep -q 'BEGIN PGP PRIVATE KEY BLOCK' "$DENTALCRAFT_GPG_KEY_FILE" 2>/dev/null; then
  gpg --batch --quiet --import "$DENTALCRAFT_GPG_KEY_FILE"
  GPG_FPR=$(gpg --batch --with-colons --list-secret-keys | awk -F: '$1 == "fpr" {print $10; exit}')
  [ "$GPG_FPR" = "$PUBKEY_FPR" ] \
    || die "the signing key $GPG_FPR is not the published release key $PUBKEY_FPR ($PUBKEY_ARMORED)"
  echo "signing key: $GPG_FPR"
elif [ "${ALLOW_UNSIGNED:-}" = 1 ]; then
  warn "no signing key (ALLOW_UNSIGNED=1): the rpm and the Flatpak bundle are NOT signed; local test only"
else
  die "no signing key: DENTALCRAFT_GPG_KEY_FILE must hold the armored secret release key (secret DENTALCRAFT_GPG_KEY)"
fi

# ---------------------------------------------------------------- staging

group "Unpack the release archives"
mkdir -p "$WORK/archives"
tar -xzf "$LINUX_ARCHIVE" -C "$WORK/archives"
unzip -q "$WINDOWS_ARCHIVE" -d "$WORK/archives"
LIN=$WORK/archives/dentalcraft_${VERSION}_linux_x86_64
WIN=$WORK/archives/dentalcraft_${VERSION}_windows_x86_64
for f in "$LIN"/{dentalcraft,dentalcraft-cli} "$WIN"/{dentalcraft.exe,dentalcraft-cli.exe} \
         "$LIN"/{README.md,LICENSE.md,THIRD-PARTY-NOTICES.md,LICENSES} "$WIN"/{README.md,LICENSE.md,THIRD-PARTY-NOTICES.md,LICENSES}; do
  [ -e "$f" ] || die "the archives lack ${f#"$WORK"/archives/}"
done
ls -l "$LIN" "$WIN"
endgroup

# The ELF objects the binaries load at start: glibc and libgcc only (the GUI libraries,
# X11, Wayland, Vulkan, EGL, are loaded at run time with dlopen). Prints the newest
# glibc symbol version they need.
check_elf() {
  local f needed
  for f in "$@"; do
    needed=$(objdump -p "$f" | awk '$1 == "NEEDED" {print $2}' | sort | tr '\n' ' ')
    for n in $needed; do
      case "$n" in
        libc.so.6|libm.so.6|libgcc_s.so.1|ld-linux-x86-64.so.2|libpthread.so.0|libdl.so.2|librt.so.1) ;;
        *) die "$f needs $n: declare it in the deb, rpm and Flatpak dependencies (packaging/README.md)" ;;
      esac
    done
  done
  objdump -T "$@" | grep -oE 'GLIBC_[0-9]+\.[0-9]+' | sed 's/GLIBC_//' | sort -Vu | tail -n 1
}
GLIBC_MIN=$(check_elf "$LIN/dentalcraft" "$LIN/dentalcraft-cli")
echo "the Linux binaries need glibc >= $GLIBC_MIN"

# Installs the Linux files of the app under DEST/PREFIX (PREFIX: usr, or empty for the
# Flatpak /app): binaries, desktop entry, icons, AppStream metadata, documentation and
# licences, and the install-kind marker read by the in-app updater.
install_linux_tree() {
  local dest=$1 prefix=$2 kind=$3 root
  root=$dest${prefix:+/$prefix}
  install -Dm755 "$LIN/dentalcraft" "$root/bin/dentalcraft"
  install -Dm755 "$LIN/dentalcraft-cli" "$root/bin/dentalcraft-cli"
  install -Dm644 "packaging/linux/$APP_ID.desktop" "$root/share/applications/$APP_ID.desktop"
  mkdir -p "$root/share/icons"
  cp -r packaging/linux/icons/hicolor "$root/share/icons/"
  install -d "$root/share/metainfo"
  sed -e "s/@VERSION@/$VERSION/" -e "s/@DATE@/$RELEASE_DATE/" \
    "packaging/linux/$APP_ID.metainfo.xml" > "$root/share/metainfo/$APP_ID.metainfo.xml"
  install -d "$root/share/doc/dentalcraft"
  cp -r "$LIN/README.md" "$LIN/LICENSE.md" "$LIN/THIRD-PARTY-NOTICES.md" "$LIN/LICENSES" "$root/share/doc/dentalcraft/"
  install -d "$root/share/dentalcraft"
  printf '%s\n' "$kind" > "$root/share/dentalcraft/install-kind"
}

# Ownership-independent, reproducible trees: plain modes, every time = SOURCE_DATE_EPOCH.
normalize_tree() {
  find "$1" -type d -exec chmod 0755 {} +
  find "$1" -type f -exec chmod u+rw,go+r,go-w {} +
  find "$1" -exec touch -h -d "@$SOURCE_DATE_EPOCH" {} +
}

group "Validate the desktop entry and the AppStream metadata"
install_linux_tree "$WORK/check" usr check
desktop-file-validate "$WORK/check/usr/share/applications/$APP_ID.desktop"
appstreamcli validate --no-net "$WORK/check/usr/share/metainfo/$APP_ID.metainfo.xml"
rm -rf "$WORK/check"
endgroup

# ---------------------------------------------------------------- formats

for f in "${FORMATS[@]}"; do
  group "Build $f"
  t0=$SECONDS
  # shellcheck source=/dev/null
  case "$f" in
    appimage) . packaging/appimage/build.sh ;;
    deb) . packaging/deb/build.sh ;;
    rpm) . packaging/rpm/build.sh ;;
    flatpak) . packaging/flatpak/build.sh ;;
    exe) . packaging/windows/build-exe.sh ;;
    msi) . packaging/windows/build-msi.sh ;;
  esac
  endgroup
  printf -- '--> %s: ok (%ss)\n' "$f" "$((SECONDS - t0))"
done

ls -l "$OUT"

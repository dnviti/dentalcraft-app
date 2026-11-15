# DentalCraft-<V>.flatpak: a single-file bundle of com.dentalcraft.DentalCraft, branch
# stable. Sourced by packaging/installers.sh; runtime and permissions are in the
# manifest packaging/flatpak/com.dentalcraft.DentalCraft.yml. The bundle carries the
# URL of the DentalCraft Flatpak repository and the release key, so that installing it
# sets up the remote that keeps it up to date; the commit is signed with the same key.
fpdir=$WORK/flatpak
install -d "$fpdir"
cp "packaging/flatpak/$APP_ID.yml" "$fpdir/"
install_linux_tree "$fpdir/app" "" flatpak
normalize_tree "$fpdir/app"
runtime_version=$(sed -n "s/^runtime-version: *'\{0,1\}\([^']*\)'\{0,1\} *$/\1/p" "$fpdir/$APP_ID.yml")
[ -n "$runtime_version" ] || die "no runtime-version in packaging/flatpak/$APP_ID.yml"
# Runtime and SDK from Flathub, in a user installation (kept in DC_PKG_CACHE, if set).
export FLATPAK_USER_DIR=$CACHE_DIR/flatpak
flatpak --user remote-add --if-not-exists flathub "$FLATHUB_REPO_URL"
flatpak --user install -y --noninteractive flathub \
  "org.freedesktop.Platform//$runtime_version" "org.freedesktop.Sdk//$runtime_version"
flatpak --user update -y --noninteractive
sign=()
[ -z "$GPG_FPR" ] || sign=(--gpg-sign="$GPG_FPR" --gpg-homedir="$GNUPGHOME")
flatpak-builder --user --disable-rofiles-fuse --force-clean --default-branch=stable \
  --state-dir="$fpdir/state" --repo="$fpdir/repo" "${sign[@]}" "$fpdir/build" "$fpdir/$APP_ID.yml"
keys=()
if [ -n "$GPG_FPR" ]; then
  keys=(--gpg-keys="$PUBKEY_BINARY")
else
  warn "the Flatpak bundle is NOT signed and carries no key"
fi
flatpak build-bundle "${keys[@]}" --repo-url="$FLATPAK_REPO_URL" --runtime-repo="$FLATHUB_REPO_URL" \
  "$fpdir/repo" "$OUT/DentalCraft-$VERSION.flatpak" "$APP_ID" stable
cat "$fpdir/build/metadata"

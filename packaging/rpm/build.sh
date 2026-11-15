# dentalcraft-<V>-1.x86_64.rpm (package dentalcraft, Version <PKG_VERSION>, Release 1),
# signed with the release key. Sourced by packaging/installers.sh; spec:
# packaging/rpm/dentalcraft.spec. The file name keeps the tag version: GitHub turns the
# tilde of a release asset name into a dot. The package also installs the DentalCraft
# dnf repository (%config(noreplace), byte-identical to
# https://dentalcraft.app/linux/dentalcraft.repo), so that dnf keeps it up to date.
rpmstage=$WORK/rpm-stage
install_linux_tree "$rpmstage" usr rpm
# rpm layout: the licences in /usr/share/licenses, the readme in /usr/share/doc.
install -d "$rpmstage/usr/share/licenses/dentalcraft"
mv "$rpmstage/usr/share/doc/dentalcraft/LICENSE.md" "$rpmstage/usr/share/doc/dentalcraft/THIRD-PARTY-NOTICES.md" \
  "$rpmstage/usr/share/doc/dentalcraft/LICENSES" "$rpmstage/usr/share/licenses/dentalcraft/"
install -Dm644 packaging/linux/dentalcraft.repo "$rpmstage/etc/yum.repos.d/dentalcraft.repo"
normalize_tree "$rpmstage"
rpmbuild -bb --quiet \
  --define "_topdir $WORK/rpmbuild" \
  --define "pkg_version $PKG_VERSION" \
  --define "stage $rpmstage" \
  packaging/rpm/dentalcraft.spec
rpmfile=$WORK/rpmbuild/RPMS/x86_64/dentalcraft-$PKG_VERSION-1.x86_64.rpm
if [ -n "$GPG_FPR" ]; then
  rpmsign --addsign --define "_gpg_name $GPG_FPR" --define "_gpg_path $GNUPGHOME" "$rpmfile"
  # The signature must verify against the published key (this container's keyring is
  # thrown away with it).
  rpmkeys --import "$PUBKEY_ARMORED"
  rpmkeys --checksig --verbose "$rpmfile"
  rpmkeys --checksig "$rpmfile" | grep -q 'signatures OK' || die "the rpm signature does not verify"
else
  warn "the rpm is NOT signed"
fi
cp "$rpmfile" "$OUT/dentalcraft-$VERSION-1.x86_64.rpm"
rpm -qpi "$OUT/dentalcraft-$VERSION-1.x86_64.rpm"
rpm -qp --requires --recommends "$OUT/dentalcraft-$VERSION-1.x86_64.rpm"

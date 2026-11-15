# dentalcraft_<V>_amd64.deb (package dentalcraft, Version <PKG_VERSION>). Sourced by
# packaging/installers.sh. The file name keeps the tag version: GitHub turns the tilde
# of a release asset name into a dot. The package also installs the DentalCraft apt
# repository (a conffile, byte-identical to https://dentalcraft.app/linux/dentalcraft.list)
# and its keyring, so that apt keeps it up to date. Depends: what the binaries link
# (glibc, libgcc); Recommends: the GUI libraries they load at run time, the
# desktop portal (file dialogs) and the Vulkan drivers.
debroot=$WORK/deb
install_linux_tree "$debroot" usr deb
install -Dm644 packaging/linux/dentalcraft.list "$debroot/etc/apt/sources.list.d/dentalcraft.list"
install -Dm644 "$PUBKEY_BINARY" "$debroot/usr/share/keyrings/dentalcraft.gpg"
cat > "$debroot/usr/share/doc/dentalcraft/copyright" <<EOF
Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Upstream-Name: DentalCraft
Upstream-Contact: DentalCraft Releases <releases@dentalcraft.app>
Source: https://dentalcraft.app/

Files: *
Copyright: 2026 Daniele Viti
License: LicenseRef-DentalCraft-EULA
 Proprietary software, free of charge for non-clinical use: see
 /usr/share/doc/dentalcraft/LICENSE.md (DentalCraft end-user licence agreement).
 The open source components and their licences are listed in
 /usr/share/doc/dentalcraft/THIRD-PARTY-NOTICES.md and /usr/share/doc/dentalcraft/LICENSES.
EOF
install -d "$debroot/DEBIAN"
printf '/etc/apt/sources.list.d/dentalcraft.list\n' > "$debroot/DEBIAN/conffiles"
cat > "$debroot/DEBIAN/control" <<EOF
Package: dentalcraft
Version: $PKG_VERSION
Architecture: amd64
Maintainer: DentalCraft Releases <releases@dentalcraft.app>
Installed-Size: $(du -sk --apparent-size --exclude=DEBIAN "$debroot" | cut -f1)
Depends: libc6 (>= $GLIBC_MIN), libgcc-s1
Recommends: libxkbcommon0, libwayland-client0, libwayland-egl1, libx11-6, libx11-xcb1, libxcursor1, libxi6, libxrandr2, libxkbcommon-x11-0, libvulkan1, mesa-vulkan-drivers | vulkan-icd, libegl1, xdg-desktop-portal
Section: science
Priority: optional
Homepage: https://dentalcraft.app/
Description: Dental CAD for the practice and the lab
 DentalCraft is an application for digital dentistry, starting with clear
 aligner planning, from the intraoral scan to the print. It includes
 dentalcraft-cli, the command line and MCP server that drive it headless or
 through a running app.
 .
 DentalCraft is not a medical device: do not use it on patients or for
 clinical decisions. Free of charge for non-clinical use under the DentalCraft
 end-user licence agreement.
EOF
normalize_tree "$debroot"
dpkg-deb --root-owner-group -Zxz --build "$debroot" "$OUT/dentalcraft_${VERSION}_amd64.deb"
dpkg-deb --info "$OUT/dentalcraft_${VERSION}_amd64.deb"

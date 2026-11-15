# DentalCraft-<V>-x86_64.msi, built with wixl from packaging/windows/dentalcraft.wxs.
# Sourced by packaging/installers.sh. The files are those of the Windows archive, plus
# the icon, with install-kind "msi"; wixl-heat lists them as the AppFiles component group.
msistage=$WORK/msi-stage
install -d "$msistage"
cp -r "$WIN/." "$msistage/"
printf 'msi\n' > "$msistage/install-kind"
cp packaging/windows/dentalcraft.ico "$msistage/"
normalize_tree "$msistage"
(cd "$msistage" && find . -type f | sed 's|^\./||' | LC_ALL=C sort) \
  | wixl-heat --var var.SourceDir --directory-ref INSTALLDIR --component-group AppFiles --win64 -p "" \
  | grep -v '^<?ignore' > "$WORK/msi-files.wxs"
wixl -a x64 -D Win64=yes -D "Version=$VERSION" -D "MsiVersion=$WIN_VERSION" -D "SourceDir=$msistage" \
  -o "$OUT/DentalCraft-$VERSION-x86_64.msi" packaging/windows/dentalcraft.wxs "$WORK/msi-files.wxs"
msiinfo suminfo "$OUT/DentalCraft-$VERSION-x86_64.msi"
msiinfo export "$OUT/DentalCraft-$VERSION-x86_64.msi" Property | grep -E '^(ProductName|ProductVersion|UpgradeCode|ALLUSERS|ARP)'

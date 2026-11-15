# DentalCraft-<V>-setup.exe, built with makensis from packaging/windows/dentalcraft.nsi.
# Sourced by packaging/installers.sh. The files are those of the Windows archive, plus
# the icon, with install-kind "exe". The licence page shows LICENSE.md, saved with a
# UTF-8 byte order mark so that NSIS reads it as UTF-8.
exestage=$WORK/exe-stage
install -d "$exestage"
cp -r "$WIN/." "$exestage/"
printf 'exe\n' > "$exestage/install-kind"
cp packaging/windows/dentalcraft.ico "$exestage/"
{ printf '\357\273\277'; cat "$WIN/LICENSE.md"; } > "$exestage/LICENSE.txt"
normalize_tree "$exestage"
makensis -V2 -INPUTCHARSET UTF8 \
  "-DVERSION=$VERSION" "-DWIN_VERSION=$WIN_VERSION" "-DSTAGE=$exestage" \
  "-DOUTFILE=$OUT/DentalCraft-$VERSION-setup.exe" packaging/windows/dentalcraft.nsi
file "$OUT/DentalCraft-$VERSION-setup.exe"

# DentalCraft-<V>-x86_64.AppImage. Sourced by packaging/installers.sh.
# AppDir: usr/ as in the deb, plus AppRun, the desktop entry and the icon at the top.
# appimagetool and the static type2 runtime (no libfuse2 needed on the host) are
# pinned in installers.sh. The in-app updater replaces the file named by $APPIMAGE.
appdir=$WORK/AppDir
install_linux_tree "$appdir" usr appimage
install -m755 packaging/appimage/AppRun "$appdir/AppRun"
cp "packaging/linux/$APP_ID.desktop" "$appdir/$APP_ID.desktop"
cp "packaging/linux/icons/hicolor/256x256/apps/$APP_ID.png" "$appdir/$APP_ID.png"
ln -s "$APP_ID.png" "$appdir/.DirIcon"
normalize_tree "$appdir"
appimagetool=$(fetch "$APPIMAGETOOL_URL" "$APPIMAGETOOL_SHA256")
runtime=$(fetch "$APPIMAGE_RUNTIME_URL" "$APPIMAGE_RUNTIME_SHA256")
chmod +x "$appimagetool"
# No FUSE in the container: appimagetool, itself an AppImage, runs extracted.
# --no-appstream: installers.sh validated the metadata already.
APPIMAGE_EXTRACT_AND_RUN=1 ARCH=x86_64 "$appimagetool" --no-appstream \
  --runtime-file "$runtime" "$appdir" "$OUT/DentalCraft-$VERSION-x86_64.AppImage"
"$OUT/DentalCraft-$VERSION-x86_64.AppImage" --appimage-extract-and-run --cli --version

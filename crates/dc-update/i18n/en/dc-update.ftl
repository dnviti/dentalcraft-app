# DentalCraft updates (English, base language).

## Commands (dentalcraft-cli, MCP, app bridge)
app-cmd-update-check = Check for updates
app-cmd-update-check-doc = Asks the DentalCraft website for the newest version and says how this installation is updated. Output: current, latest, update_available, kind (standalone, appimage, deb, rpm, flatpak, exe, msi, macos-app or unknown), action (install, package-manager, manual or none) and notes_url; with package-manager also command, the command to run; with install also download (url, size); with manual also download_page. Without a connection it fails and nothing changes.
app-cmd-update-apply = Update DentalCraft
app-cmd-update-apply-doc = Installs the newest version when DentalCraft can do it by itself (standalone folder, AppImage, Windows setup or MSI, macOS app): it is downloaded, checked to be genuine and complete, and put in place; if any check fails the installed version stays as it is. Installations of apt, dnf and Flatpak are updated by their package manager: nothing is downloaded and the output gives the command. In dentalcraft-cli the new version is installed at once (a Windows installer runs when the command ends). In the app it is installed when the app closes, or at once with restart set to true: the project is saved, the app closes and the new version opens. Output: the one of app.update.check plus updated.

## Errors
update-error-offline = DentalCraft could not reach its website. Check the internet connection and try again.
update-error-server = The DentalCraft website did not answer as expected. Try again later.
update-error-not-genuine = The update could not be confirmed as coming from DentalCraft, so it was ignored. Nothing was installed.
update-error-damaged = The downloaded file is damaged or incomplete, so it was deleted and nothing was installed. Try again.
update-error-cancelled = The download was stopped. Nothing was installed.
update-error-not-writable = DentalCraft cannot add files to { $path }. Download the new version from the website, or move DentalCraft to a folder you can change.
update-error-no-download = There is no download of the new version for this copy of DentalCraft. Get it from the website.
update-error-install = The new version could not be installed. The version you have is unchanged.

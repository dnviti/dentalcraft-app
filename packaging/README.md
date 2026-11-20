# Packaging

This folder turns the two standalone release archives into the DentalCraft installers. Every package holds the same `dentalcraft` (the app) and `dentalcraft-cli` (the command line and MCP server) binaries as the archives. It also carries the licence (`LICENSE.md`), the third-party notices (`THIRD-PARTY-NOTICES.md`, `LICENSES/`) and an `install-kind` marker for the in-app updater.

| Release asset | Kind | Installs | Updates through |
|---|---|---|---|
| `dentalcraft_<V>_linux_x86_64.tar.gz` | standalone | nothing: unpack and run | the in-app updater |
| `dentalcraft_<V>_windows_x86_64.zip` | standalone | nothing: unpack and run | the in-app updater |
| `DentalCraft-<V>-x86_64.AppImage` | AppImage | nothing: one executable file | the in-app updater |
| `dentalcraft_<V>_amd64.deb` | deb | system-wide, `/usr` | apt (the package adds the DentalCraft repository) |
| `dentalcraft-<V>-1.x86_64.rpm` | rpm | system-wide, `/usr` | dnf (the package adds the DentalCraft repository) |
| `DentalCraft-<V>.flatpak` | Flatpak bundle | `flatpak install` | the DentalCraft Flatpak remote the bundle sets up |
| `DentalCraft-<V>-setup.exe` | NSIS setup | for the current user, no administrator rights | the in-app updater |
| `DentalCraft-<V>-x86_64.msi` | MSI | for all users (administrator rights) | the in-app updater |
| `dentalcraft_<V>_macos_universal.zip` | macOS app bundle (`DentalCraft.app`, Apple Silicon and Intel) | drag `DentalCraft.app` to `/Applications` | the in-app updater (**not built while the macOS job is disabled**, see macOS) |
| `update.json`, `update.json.minisig` | update manifest | — | read by the in-app updater, served at `https://dentalcraft.app/update/stable.json` |
| `dentalcraft_<V>_SHA256SUMS.txt` | checksums of every asset above | — | — |

`<V>` is the tag without the `v`, for example `0.2.0-alpha.4`.

## How they are built

`.github/workflows/binaries.yml` runs on every release, and `scripts/release-local.sh binaries app <tag>` of the superproject replays it locally while GitHub Actions is suspended (`docs/local-ci.md` there). It has three jobs:

1. **build** (matrix `linux_x86_64`, `windows_x86_64`): `cargo build --release`, then the standalone archive. Each archive holds the binaries, `README.md`, `LICENSE.md`, `THIRD-PARTY-NOTICES.md`, `LICENSES/` (font licences) and `install-kind` (`standalone`), in one top folder `dentalcraft_<V>_<target>/`.
2. **installers**: `packaging/installers.sh` builds the six installers from the two archives. It runs as root in a throwaway `registry.fedoraproject.org/fedora:45` container started with `--privileged`, because flatpak-builder sandboxes its build with bubblewrap. It installs its tools there with dnf.
3. **release**: `packaging/update-manifest.py` writes and signs `update.json`. Then the job writes `dentalcraft_<V>_SHA256SUMS.txt`, uploads everything to the GitHub release and appends the download notes.

Run the installers by hand, from the root of a checkout that has the two archives in `dist/`:

```sh
podman run --rm --privileged -v "$PWD:/work/dentalcraft-app" -w /work/dentalcraft-app \
  -e TAG=v0.2.0-alpha.4 -e SOURCE_DATE_EPOCH="$(git log -1 --format=%ct)" -e ALLOW_UNSIGNED=1 \
  registry.fedoraproject.org/fedora:45 bash packaging/installers.sh deb rpm   # any subset; default: all six
```

Outputs go to `installers/`. Without a signing key the run fails, unless `ALLOW_UNSIGNED=1` (local tests only). With a key, pass the armored secret key as a mounted file: `-v key.asc:/run/secrets/key:ro -e DENTALCRAFT_GPG_KEY_FILE=/run/secrets/key`. The superproject script does all of this for you: `scripts/release-local.sh binaries app <tag> --no-upload` builds every asset into `~/.cache/dentalcraft-ci/app-binaries/dist`.
- `--src <working tree>` builds uncommitted sources instead of the tag.
- `--only installers` reruns only this job on the archives of the last run.
- `--no-sign` builds without the release keys.

### Tools

| Tool | Version | Source |
|---|---|---|
| appimagetool | 1.9.1 | GitHub release, sha256 pinned in `installers.sh` |
| AppImage runtime (type2, static: no libfuse2 needed on the user's system) | 20251108 | GitHub release, sha256 pinned in `installers.sh` |
| rpm-build, rpm-sign | 6.1.0 | Fedora 45 |
| dpkg (dpkg-deb) | 1.23.11 | Fedora 45 |
| flatpak, flatpak-builder | 1.18.4, 1.4.12 | Fedora 45 |
| NSIS (makensis) | 3.12 (`mingw32-nsis`) | Fedora 45 |
| msitools (wixl, wixl-heat) | 0.106.73 | Fedora 45 |
| squashfs-tools, appstream, desktop-file-utils | 4.7.4, 1.1.3, 0.28 | Fedora 45 |

The job prints the installed Fedora package versions. Fedora 45 only takes bug-fix updates. The packages are reproducible for a given tag: every file time is `SOURCE_DATE_EPOCH`, the commit time of the tag.

## Versions

- Package names keep `<V>`. The metadata of the deb and the rpm puts the pre-release after a tilde, which sorts before the release: deb `Version: 0.2.0~alpha.4`, rpm `Version: 0.2.0~alpha.4`, `Release: 1`. File names cannot carry the tilde, because GitHub turns `~` into `.` in release asset names. The apt and dnf repositories index the packages by their metadata, not by file name.
- Windows wants four numbers. The pre-release number becomes the fourth: `0.2.0-alpha.4` gives `0.2.0.4`, and `0.2.0` gives `0.2.0.0`. MSI upgrades compare only the first three numbers (see MSI).
- The AppStream release entry and the Windows display version carry `<V>` itself.

## `install-kind`

A UTF-8 text file with one word and a newline, which tells the in-app updater how this copy was installed:

| Kind | File |
|---|---|
| `standalone` | `install-kind` next to the binaries, in the archive's top folder |
| `exe`, `msi` | `install-kind` next to `dentalcraft.exe` in the install folder |
| `appimage` | `usr/share/dentalcraft/install-kind` inside the AppImage |
| `deb`, `rpm` | `/usr/share/dentalcraft/install-kind` |
| `flatpak` | `/app/share/dentalcraft/install-kind` |
| `macos-app` | `Contents/Resources/install-kind` inside `DentalCraft.app` (files in `Contents/MacOS/` must be code) |

The app reads `<exe dir>/install-kind`, then `<exe dir>/../share/dentalcraft/install-kind`, then `<exe dir>/../Resources/install-kind` (the macOS bundle). A set `$APPIMAGE` means `appimage`, and the file to replace is the one it names. An existing `/.flatpak-info` means `flatpak`. These two checks win over the file.

## Linux

The desktop entry, the AppStream metadata (`packaging/linux/com.dentalcraft.DentalCraft.*`) and the icons are shared by all Linux formats:
- The icons are hicolor 16 to 512 px plus scalable SVG, from `dentalcraft-web/brand` (`png/app-icon/dentalcraft-app-icon-<size>.png`, `svg/dentalcraft-app-icon-full.svg`).
- The app id `com.dentalcraft.DentalCraft` is also the Wayland app id and the X11 `WM_CLASS` of the window.
- `installers.sh` validates both files and fills in the release version and date.

**Compatibility.** The Linux binaries are built on the runner image (Ubuntu 24.04), so they need glibc 2.39 or newer. That covers Debian 13, Ubuntu 24.04, Fedora 40 and later. `installers.sh` derives the minimum glibc from the binaries. It fails if they link anything other than glibc and libgcc. The GUI libraries (Wayland, X11, xkbcommon, Vulkan, EGL) are loaded at run time, so the packages list them as recommended, not required. The command line runs without them.

### AppImage

- **AppDir:** `usr/` as in the deb, `AppRun`, the desktop entry and the 256 px icon.
- **Running:** `DentalCraft-<V>-x86_64.AppImage` starts the app. `DentalCraft-<V>-x86_64.AppImage --cli <args>` runs `dentalcraft-cli`, and so does a symlink to the AppImage named `dentalcraft-cli`.
- **Without FUSE**, for example in a container, add `--appimage-extract-and-run`.

### deb

- **Package:** `dentalcraft`.
- **Contents:** `/usr/bin/dentalcraft`, `/usr/bin/dentalcraft-cli`, the desktop entry, icons, metainfo, and `/usr/share/doc/dentalcraft/` (README, licence, notices, `LICENSES/`, `copyright`).
- **Dependencies:** `Depends: libc6 (>= 2.39), libgcc-s1`. The GUI libraries, the Vulkan driver and `xdg-desktop-portal` (file dialogs) are `Recommends`.
- **Repository:**
  - It installs the DentalCraft apt repository as the conffile `/etc/apt/sources.list.d/dentalcraft.list`, with its keyring `/usr/share/keyrings/dentalcraft.gpg` (binary form of `packaging/keys/dentalcraft.gpg`).
  - From then on, `apt upgrade` updates it, as it does Chrome or VS Code.
  - The `.list` is byte-identical to the one the site serves (`dentalcraft-web/linux/dentalcraft.list`), so a manual repository setup followed by the package gives no conffile prompt. `release-local.sh` fails if the two copies differ.

### rpm

- **Package:** `dentalcraft`, signed with the release key (`rpmkeys --checksig` must verify it against `packaging/keys/dentalcraft.gpg`).
- **Spec:** `packaging/rpm/dentalcraft.spec`. Same contents as the deb. The licence files go in `/usr/share/licenses/dentalcraft/`.
- **Dependencies:** rpm generates `Requires` from the binaries. The GUI libraries are weak dependencies by soname, valid on any rpm distribution.
- **Payload:** xz. The binaries are neither stripped nor split into debuginfo.
- **Repository:** `/etc/yum.repos.d/dentalcraft.repo`, `%config(noreplace)`, byte-identical to `dentalcraft-web/linux/dentalcraft.repo`. It sets `gpgcheck=1`, `repo_gpgcheck=1` and the key URL `https://dentalcraft.app/keys/dentalcraft.gpg`.
- Fedora's rpm 6 checks signatures on install. Import the key first (`sudo rpm --import https://dentalcraft.app/keys/dentalcraft.gpg`) or install through the repository.

### Flatpak

- **Manifest:** `packaging/flatpak/com.dentalcraft.DentalCraft.yml`.
- **Runtime:** `org.freedesktop.Platform` 25.08, from Flathub. The 24.08 runtime is end of life.
- **Permissions:** `--device=dri` (wgpu), Wayland with X11 fallback, IPC, network (sign-in, support, update check), the Secret Service `org.freedesktop.secrets` (the sign-in session in the OS keyring, so it survives a restart), and the home folder (projects in `~/DentalCraft`, scans opened from anywhere in it).
- **Branch:** `stable`.
- **Bundle:** `flatpak build-bundle` writes it with the repository URL `https://dentalcraft.app/linux/flatpak/`, the Flathub runtime repository and the release key. The commit is signed with the same key.
- **Install:** `flatpak install --user DentalCraft-<V>.flatpak` (or system-wide) adds a remote for that repository, so `flatpak update` updates DentalCraft from then on.
- **Command line:** `flatpak run --command=dentalcraft-cli com.dentalcraft.DentalCraft --version`.
- **Site:** the site does not serve the bundle; there you install from the repository (`/linux/DentalCraft.flatpakref`).

## Windows

**Code signing (Authenticode):** `packaging/windows/authenticode.sh` signs with the DentalCraft **self-signed** certificate (`packaging/windows/dentalcraft-selfsigned.crt.pem`, SHA-256 fingerprint `24DD542E337BCEA99C793D57DE53B6D355DC015BF1270D20D1321D56DCF0AA26`), using jsign 7.6 and an RFC 3161 timestamp from `http://ts.ssl.com`. It then checks each file with `osslsigncode verify`: the signature against that certificate, the timestamp against the pinned SSL.com root (`packaging/windows/timestamp-root.pem`). `binaries.yml` runs it twice:
- on `dentalcraft.exe` and `dentalcraft-cli.exe` before they are packed;
- on setup.exe and the MSI after `installers.sh`.

Both runs come before `update.json` and the checksums. The key, the order and the tests are in the script header and in the superproject's `docs/local-ci.md`.
- **Windows does not trust a self-signed certificate.** SmartScreen still warns on the first start of the setup, the MSI and the app, and the publisher shows as unknown. The signature only lets users check which key signed the files: the release notes print the certificate's fingerprint.
- Without the `DENTALCRAFT_AUTHENTICODE_*` secrets (or, locally, `~/.config/dentalcraft-release/authenticode`), the files stay unsigned with a warning.
- The NSIS uninstaller (`uninstall.exe`, written by the setup at install time) is not signed.

### Trusting the self-signed certificate on a Windows PC

Untested on Windows: the steps follow Microsoft's documentation of `certutil` and `Import-Certificate`, and nobody has run them on a DentalCraft PC yet.

- **File:** `dentalcraft-selfsigned.cer` (DER), from the release key directory `~/.config/dentalcraft-release/authenticode/`. It is the same certificate as `packaging/windows/dentalcraft-selfsigned.crt.pem`.
- **Check the fingerprint first:** `certutil -hashfile dentalcraft-selfsigned.cer SHA256` must print `24dd542e337bcea99c793d57de53b6d355dc015bf1270d20d1321d56dcf0aa26` (upper or lower case). Do not install a file with any other value.
- **Install**, in an elevated prompt (Run as administrator):

  ```bat
  certutil -addstore -f Root dentalcraft-selfsigned.cer
  certutil -addstore -f TrustedPublisher dentalcraft-selfsigned.cer
  ```

  PowerShell equivalent: `Import-Certificate -FilePath dentalcraft-selfsigned.cer -CertStoreLocation Cert:\LocalMachine\Root`, then the same with `Cert:\LocalMachine\TrustedPublisher`.
- **Remove:** `certutil -delstore Root 35ec884c9c5a0cfb30dd86e48071cfb0f3aa7f97` and `certutil -delstore TrustedPublisher 35ec884c9c5a0cfb30dd86e48071cfb0f3aa7f97` (the certificate's serial number).
- **What trusting it allows:** the certificate is `CA:FALSE` with the code-signing EKU only, so it cannot issue other certificates. Trusting it only trusts what this key signs: the signature then shows as valid, with the publisher "DentalCraft (self-signed)".
- **What it does not change:** SmartScreen reputation does not come from local trust. A downloaded file still carries the Mark of the Web and may still show "Windows protected your PC". `Unblock-File <file>` in PowerShell, or Properties > Unblock, removes the mark.

### setup.exe (NSIS)

- **Script:** `packaging/windows/dentalcraft.nsi`. The setup is in English or Italian, following the Windows language.
- **Per user, no administrator rights:** installs in `%LOCALAPPDATA%\Programs\DentalCraft`.
- **Shortcuts:** Start menu shortcut, plus an optional desktop shortcut (Components page, kept on updates).
- **Uninstall:** an uninstaller, and the Apps & features entry `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\com.dentalcraft.DentalCraft`.
- **Silent install, used by the in-app updater:** `DentalCraft-<V>-setup.exe /S`.
  - It installs into the existing folder; `/D=<folder>`, last on the command line, chooses another.
  - It returns when done, with exit code 0 on success.
  - It waits up to 30 s for a running `dentalcraft.exe` to exit, then fails.
- **Silent uninstall:** `"%LOCALAPPDATA%\Programs\DentalCraft\uninstall.exe" /S`.

### MSI

- **Built with:** wixl, from `packaging/windows/dentalcraft.wxs`; `wixl-heat` lists the files.
- **Per machine:** `C:\Program Files\DentalCraft`, administrator rights.
- **Shortcuts:** Start menu shortcut, plus an optional desktop shortcut (feature `DesktopShortcut`, off by default).
- **Commands:**
  - `msiexec /i DentalCraft-<V>-x86_64.msi` installs; add `/passive` for a progress bar only, or `/qn` for nothing.
  - `msiexec /i DentalCraft-<V>-x86_64.msi ADDLOCAL=ALL` also adds the desktop shortcut.
  - `msiexec /x DentalCraft-<V>-x86_64.msi` removes it.
- **UpgradeCode:** `{90BDCF16-C5BE-4030-B024-F81F76394F48}`.
  - Generated once on 2026-11-15, it identifies DentalCraft across all versions. **Never change it**: a new code would install side by side instead of upgrading.
  - Each build gets a new ProductCode. `MajorUpgrade` removes any earlier version before installing.
- **Versions:**
  - `AllowSameVersionUpgrades` covers pre-releases, which differ only in the fourth version number, which MSI ignores.
  - A newer X.Y.Z already installed blocks the downgrade.

## macOS

Written and checked on Linux only (`cargo check` and `clippy` for `aarch64-apple-darwin` and `x86_64-apple-darwin`, the unit tests of the updater with a bundle on disk). Nobody has built or run the bundle on a Mac yet. The release job is disabled (below), so the asset is not published and `update.json` omits `macos-universal` (`update-manifest.py` lists it only when the file is in `--assets`).

- **Files:** `packaging/macos/`:
  - `Info.plist.in`: template with `@VERSION@` (the release version, with the pre-release) and `@BUNDLE_VERSION@` (`X.Y.Z`: `CFBundleVersion` takes at most three numbers). `LSMinimumSystemVersion` is 11.0. `NSCameraUsageDescription` and `NSBluetoothAlwaysUsageDescription` are the texts macOS shows when the app first uses the scanner (a USB camera) or the turntable (Bluetooth). Without them macOS ends the program at that moment. The Italian texts are in `InfoPlist.it.strings` (`Resources/it.lproj/`).
  - `entitlements.plist`: camera, Bluetooth and USB. With the ad-hoc signature macOS asks through `Info.plist` alone; the entitlements start to count with the hardened runtime (Developer ID and notarization, not done).
  - `bundle.sh <V>`: run on a Mac, from the root of a checkout of the tag outside the superproject. It builds `dentalcraft` and `dentalcraft-cli` for both targets (`MACOSX_DEPLOYMENT_TARGET=11.0`, `--locked`), joins each with `lipo`, writes `DentalCraft.app` (`Contents/MacOS/{dentalcraft,dentalcraft-cli}`, `Info.plist`, the icon made with `iconutil` from the Linux PNGs, `Resources/{LICENSE.md,THIRD-PARTY-NOTICES.md,README.md,LICENSES/,install-kind}`), signs the two programs and then the bundle **ad hoc** (`codesign --sign -`), checks it with `codesign --verify --deep --strict` and writes `dist/dentalcraft_<V>_macos_universal.zip` with `ditto -c -k --keepParent`. `NO_BUILD=1` reuses the binaries already built.
- **Signing:** ad hoc, no Developer ID, no notarization. Apple Silicon refuses unsigned code, so the ad-hoc signature is the minimum. Gatekeeper still asks once: System Settings > Privacy & Security > Open Anyway (a file downloaded by the in-app updater is not quarantined).
- **In-app update (`dc-update`):** the manifest signature (minisign) and the SHA-256 and size of the `.zip` are checked as for every other asset. The `.zip` is unpacked next to the app (`.DentalCraft.app.update/staged/`, same volume), only one `<name>.app` folder is accepted (no `..`, no links, no `__MACOSX`), the quarantine flag is removed and `codesign --verify --deep --strict` must pass. When the app closes, the installed bundle is renamed into `previous/` and the new one to its place (undone if the second rename fails); with restart, `open -n` opens the new app. The old bundle is removed at the next start. If the folder of the app is not writable (a read-only disk image, or an app translocated by Gatekeeper), the update fails before downloading, and the message tells the user to download the new version or move the app to a folder they can change.
- **What a Mac must still confirm:**
  - `bundle.sh` runs to the end, and `lipo -info` reports `x86_64 arm64` for both programs;
  - Gatekeeper opens the ad-hoc signed app after Open Anyway, on Apple Silicon and on Intel;
  - the camera and Bluetooth questions appear, and macOS remembers the answers for `DentalCraft.app`;
  - an update from a published `.zip` replaces the running app and `open -n` starts the new one.

### Enabling the macOS job (not applied)

`binaries.yml` is not touched here (Actions are suspended; the macOS job stays `if: false`). To publish the macOS asset with this script, the job and the release job would change like this:

1. `build-macos`: remove `if: false`; delete the two `Build aarch64` / `Build x86_64` steps and the inline `Bundle DentalCraft.app` step, and replace them with one step (the script builds both targets, with `rustup target add` and `MACOSX_DEPLOYMENT_TARGET=11.0`; the `Install Rust` step with `targets:` stays):

   ```yaml
         - name: Bundle DentalCraft.app (universal, ad-hoc signed, not notarized)
           env:
             TAG: ${{ inputs.tag }}
           run: bash packaging/macos/bundle.sh "${TAG#v}"
   ```

   `bundle.sh` writes `DentalCraft.app` to `target/macos/` and only the `.zip` to `dist/`, so the existing `upload-artifact` step (`path: dist/*`) uploads just the `.zip`.
2. `release`: add `build-macos` to `needs`; add `dentalcraft_${VERSION}_macos_universal.zip` to `assets` in "Checksums" (the check for unexpected files would otherwise fail); `update-manifest.py` needs no change, it lists `macos-universal` when the file is present.
3. "Release notes": append to the standalone line `; macOS universal (Apple Silicon and Intel): \`.zip\` with \`DentalCraft.app\`` as the comment there says, and add the line about the ad-hoc signature and Open Anyway.
4. The runner is `macos-latest` (billed); `scripts/release-local.sh` has no macOS step: a Mac is needed to produce the `.zip`.

## Keys and secrets

- **Release GPG key:** `DentalCraft Releases <releases@dentalcraft.app>`, ed25519, fingerprint `83B0A31CA19DE14A1648A875C10CE2EB009DE538`.
  - Public key: `packaging/keys/dentalcraft.gpg`, a copy of `dentalcraft-web/keys/dentalcraft.gpg`.
  - It signs the rpm, the Flatpak commit, and on the site the apt, rpm and Flatpak repositories.
  - `installers.sh` refuses a signing key whose fingerprint differs from the published one.
- **Minisign key:** signs `update.json`. The app embeds its public key.
- **GitHub secrets of this repository** (the local script uses `~/.config/dentalcraft-release/` instead):
  - `DENTALCRAFT_GPG_KEY`: the armored secret release key (`gpg --armor --export-secret-keys 83B0A31CA19DE14A1648A875C10CE2EB009DE538`).
  - `DENTALCRAFT_MINISIGN_KEY`: the contents of the unencrypted minisign secret key file.

## Update manifest

`packaging/update-manifest.py --version <V> --assets dist [--date YYYY-MM-DD]` reads the minisign key from `DENTALCRAFT_MINISIGN_KEY` (or `--key FILE`). It writes `dist/update.json` and `dist/update.json.minisig`. The manifest holds version, tag, date, notes URL and, for `linux-standalone`, `appimage`, `windows-standalone`, `windows-exe` and `windows-msi`, the download URL on `https://dentalcraft.app/download/`, the sha256 and the size. The deb, rpm and Flatpak update through their repositories.

# dentalcraft-app

The shipped DentalCraft desktop application: a Rust workspace with the eframe/wgpu shell, the 3D viewport, the authentication client and the packaging tooling. Depends on `dentalcraft-core` and `dentalcraft-aligners`.

This repository is a submodule of the [`dentalcraft`](https://github.com/dnviti-personal-org/dentalcraft) superproject, which holds the plan, the architecture decisions and the pinned versions of all DentalCraft repositories. Clone the superproject with `--recurse-submodules` to work on the whole project.

- Visibility: private.
- Versioning: [release-please](https://github.com/googleapis/release-please) on `main` (Conventional Commits, tags `vX.Y.Z`).
- CI: GitHub Actions on the `golgari-dev` runner scale set.

## Updates

The app and `dentalcraft-cli` update themselves from the DentalCraft website (crate [`dc-update`](crates/dc-update/src/lib.rs)).

- **When.** At startup (account menu > **Check for updates at startup**, on by default), from the account menu > **Check for updates…**, and from **Support** > Your workstation. A check at startup that fails (offline) says nothing; one the user asked for says why.
- **What is trusted.** The website publishes `https://dentalcraft.app/update/stable.json` and its minisign signature `stable.json.minisig`, made by the release job with `packaging/update-manifest.py`. The app checks the signature with the release key built into it (`dc_update::PUBLIC_KEY`, key id `B155D07CF2A36DB0`, `keys/dentalcraft.minisign.pub` on the website) before it uses anything in the file, compares the versions by SemVer (a pre-release comes before its release), and checks every download against the size and SHA-256 the signed file gives. When a check fails nothing is installed. `DC_UPDATE_URL` points the check at another address (a test site); the key never changes.
- **How.** Each package writes a marker file `install-kind` (one word: `standalone`, `appimage`, `deb`, `rpm`, `flatpak`, `exe`, `msi`) next to the program, or in `../share/dentalcraft/` from its folder; `$APPIMAGE` and `/.flatpak-info` win over it.

  | Installation | Update now |
  |---|---|
  | standalone folder (`.tar.gz`, `.zip`) | the archive is unpacked into `<folder>/.update/staged`; when the app closes its entries take the place of the old ones, which go to `<folder>/.update/previous` (removed at the next start; a failed swap is undone) |
  | AppImage | the new file is downloaded next to `$APPIMAGE` and renamed over it when the app closes |
  | Windows setup (`.exe`) or MSI | the installer is downloaded and runs once the app has quit: `setup.exe /S /D=<folder>` or `msiexec /i <file> /passive /norestart` |
  | deb, rpm, Flatpak | nothing is downloaded: the dialog gives the command (`sudo apt update && sudo apt upgrade dentalcraft`, `sudo dnf upgrade dentalcraft`, `flatpak update com.dentalcraft.DentalCraft`) |
  | no marker (a development build) | the dialog opens the download page |

- **The dialog.** It shows the new version, a "What's new" link and **Update now**, **Later**, **Skip this version** (the check at startup then stays quiet for that version). The download shows its progress and can be cancelled or hidden. When it is ready, **Restart now** saves the open project as closing the window does (a project never saved asks first), installs the new version and starts it; **Later** installs it whenever DentalCraft closes. Downloads wait in the data folder (`updates/`, e.g. `~/.local/share/dentalcraft/updates`).
- **Agents.** `app.update.check` (current, latest, kind, action, ...) and `app.update.apply` (`restart: true` restarts at once) in the app bridge and in `dentalcraft-cli` and its MCP server; on the command line `dentalcraft-cli update --check` and `dentalcraft-cli update`.

## Licence

DentalCraft is proprietary software: copyright (c) 2026 Daniele Viti, all rights reserved. The source code is private. The application is free of charge to download and use, for non-clinical purposes, under the DentalCraft end-user licence agreement in [`LICENSE.md`](LICENSE.md) (Italian and English); it is provided as is, without warranty.

DentalCraft is not a medical device: do not use it on patients or for clinical decisions.

DentalCraft includes open source components, each under its own licence; their copyright and licence notices are in [`THIRD-PARTY-NOTICES.md`](THIRD-PARTY-NOTICES.md). Both texts are built into the programs, so a downloaded copy always carries them:

- in the app: account menu (top right) > **Licence and notices**, or **Terms of use** at the bottom of the sign-in screen;
- on the command line: `dentalcraft-cli licence` prints the licence, `dentalcraft-cli licence --third-party` the third-party notices, and `dentalcraft-cli --version` the copyright line.

# dentalcraft-app

The desktop application of [DentalCraft](https://dentalcraft.app), a tool for digital dentistry, written in Rust. This repository holds the application shell (eframe and wgpu), the 3D viewport, the module workspace, the scan popup, the sign-in client, the updater, the anonymous diagnostics, `dentalcraft-cli` (command line and MCP server) and the packaging scripts.

It depends on [`dentalcraft-core`](https://github.com/dnviti/dentalcraft-core) (engine) and [`dentalcraft-aligners`](https://github.com/dnviti/dentalcraft-aligners) (the first module).

> **Status: alpha.** Screens, commands and the project format can still change between releases. The scanner connector is in alpha too.
>
> **Not a medical device.** DentalCraft is not certified and must not be used on patients or for clinical decisions. See [Disclaimer](#disclaimer).

## What it does

| Area | What you get |
|---|---|
| Home and new project | Start from STL, OBJ or PLY scans, or scan with a connected 3D scanner. Projects open without an account. |
| Workspace | Menu of functions grouped by what they are for, an inspector with the parameters and tools of the selected function, a 3D viewport with standard dental views and picking, and a timeline of the stages. |
| Aligners module | Teeth, diagnosis, bite, final position, aligners, review, production and follow-up, from the scans to printable models. |
| Scan popup (alpha) | Drives a 3D scanner (Revopoint MINI 2) and a turntable, cleans the point cloud, builds and repairs the mesh, and hands it to the screen that opened it. |
| Projects | `.dcprj` files with undo, automatic saving and a step history. |
| Languages | English and Italian, switchable at run time. |
| Account | Sign-in with OpenID Connect against the DentalCraft account server (not part of the public sources). Local projects open without it. |
| Agents | `dentalcraft-cli` runs every command headless and serves them over MCP. `dentalcraft --control <port>` lets the CLI drive a running window. |
| Updates and diagnostics | Signed automatic updates, and anonymous diagnostics only after an explicit opt-in. |

| Crate | What it does |
|---|---|
| `dc-app` | The application: Home, sign-in, support, new project, the module workspace, the "Licence and notices" dialog. |
| `dc-workspace` | Workspace screen: menu tree, viewport, inspector, stage timeline, status bar, import and save. |
| `dc-viewport` | 3D viewport: wgpu mesh rendering inside egui, orbit camera, standard dental views, CPU picking. |
| `dc-scan-ui` | The scan popup, reusable by any screen or module. |
| `dc-ui` | Design system: tokens, fonts, Phosphor icons, theme, shared widgets. |
| `dc-auth-client` | Desktop sign-in (OIDC authorization code with PKCE, keyring storage of the refresh token). |
| `dc-update` | Update check, signature check and installation. |
| `dc-diagnostics` | Anonymous crash, error and session reports, queued on disk. |
| `dc-cli` | `dentalcraft-cli`: command line and MCP server. |

## Install

Builds and installers for Linux (Flatpak, dnf, apt, AppImage, standalone archive) and Windows (setup `.exe`, `.msi`, standalone archive) are on the [download page](https://dentalcraft.app/download/). The installed application updates itself. The packaging scripts are in [`packaging/`](packaging/README.md). A macOS bundle exists in the scripts but is not built at the moment.

## Build, run and test

You need the Rust toolchain pinned in [`rust-toolchain.toml`](rust-toolchain.toml). No C++ toolchain is needed. On Linux you also need the usual GUI development libraries (Wayland or X11, Vulkan or OpenGL). The dependencies on core and aligners are git dependencies pinned by tag in `Cargo.toml`.

```sh
cargo run -p dc-app --release          # the application
cargo run -p dc-cli -- --help          # dentalcraft-cli
cargo build --release --workspace --bins

cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

A debug build signs in against a local server at `http://127.0.0.1:8787`; a release build uses the production server. `DC_AUTH_ISSUER` overrides the address. Tests run headless: only logic tests (`egui_kittest`) and no GPU are needed.

To change core, aligners and the app together, work in the [`dentalcraft`](https://github.com/dnviti/dentalcraft) superproject, which redirects the dependencies to the working trees. See [`CONTRIBUTING.md`](CONTRIBUTING.md).

## Architecture

- [`dentalcraft`](https://github.com/dnviti/dentalcraft) (superproject): pins all repositories and holds the architecture decision records in `docs/adr`.
- [`dentalcraft-docs`](https://github.com/dnviti/dentalcraft-docs): documentation.
- Layering: core has no DentalCraft dependency; aligners depends on core; this application depends on both.

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

## Anonymous diagnostics

With the user's consent, the app sends anonymous reports of crashes and errors to the DentalCraft server (crate [`dc-diagnostics`](crates/dc-diagnostics/src/lib.rs)), so bugs can be found and fixed.

- **Consent.** At the first start (and at each start while unanswered) a dialog explains what is sent and what never is, with an unticked box and **Continue**; account menu > **Send anonymous diagnostic data** changes it at any time. It is stored in `settings.json` as `diagnostics` (`null` until answered) and `install_id`, a random UUID not linked to the account, made when diagnostics are turned on and deleted, with the reports not sent yet, when they are turned off. Nothing is collected or sent before the box is ticked.
- **What.** One `session` event per start (`startup_ms`); `crash` events from the panic hook (message, backtrace, last log lines; crash files `crash-<time>.log` of earlier runs are picked up at start and renamed `crash-<time>.reported.log`); `error` events for every `ERROR` log line and `warning` events for `WARN` lines flagged `diag = true`, with the structured fields `code`, `command`, `module`, `source`. Version, commit, installation kind, OS, architecture, language, GPU, memory and processors go with each report. Never scans, projects, patient data or account data: home folder, user name, project and scan file names and their folders, registered patient codes and case references, e-mail addresses and access codes are removed from every text first, and texts are cut to the contract's limits.
- **Where.** `POST {issuer}/api/v1/diagnostics/reports` (the sign-in server: `DC_AUTH_ISSUER`, the saved setting, else the build default), without any `Authorization`. Events wait in `<data>/diagnostics/queue` (at most 500 events and 5 MiB, oldest dropped) and are sent at start, a few seconds after new ones and every ten minutes; after a `429`, a server error or no connection they wait (`Retry-After`, else 60 s doubling to an hour); a `400` or `413` drops them.
- **Seeing it.** Support > Your workstation says whether diagnostics are on and shows the next report (**Show what is sent**).
- **Agents.** `app.diagnostics.status`, `app.diagnostics.set {enabled}`, `app.diagnostics.preview` and `app.diagnostics.send` in the app bridge and in `dentalcraft-cli` and its MCP server (the CLI works on the app's settings file and queue).

## Licence

Source-available under the [PolyForm Noncommercial License 1.0.0](LICENSE.md). You may read, use, change and share the code, and use the application, for any noncommercial purpose. Commercial use is not allowed. The applications are free to download for noncommercial use.

This is not an open source licence in the OSI sense. The third-party components keep their own licences: see [`NOTICE.md`](NOTICE.md) and [`THIRD-PARTY-NOTICES.md`](THIRD-PARTY-NOTICES.md). All of these texts are built into the programs, so a downloaded copy always carries them:

- in the app: account menu (top right) > **Licence and notices**, or **Terms of use** at the bottom of the sign-in screen;
- on the command line: `dentalcraft-cli licence` prints the licence and the notice, `dentalcraft-cli licence --third-party` the third-party notices, and `dentalcraft-cli --version` the copyright line.

**Commercial licensing.** If you want to use DentalCraft commercially, contact the author at hello@dentalcraft.app.

## Disclaimer

DentalCraft is not a certified medical device and has no regulatory clearance. It is provided as is, without warranty. Do not use it on patients or for clinical decisions without independent professional validation. See [`SECURITY.md`](SECURITY.md).

## Contributing

Read [`CONTRIBUTING.md`](CONTRIBUTING.md) (DCO sign-off, inbound licence grant, checks) and the [Code of Conduct](CODE_OF_CONDUCT.md). Report security problems privately, as described in [`SECURITY.md`](SECURITY.md).

## Links

| | |
|---|---|
| This repository | https://github.com/dnviti/dentalcraft-app |
| Superproject | https://github.com/dnviti/dentalcraft |
| Engine | https://github.com/dnviti/dentalcraft-core |
| Aligners module | https://github.com/dnviti/dentalcraft-aligners |
| Website | https://github.com/dnviti/dentalcraft-web |
| Documentation | https://github.com/dnviti/dentalcraft-docs |
| Product site | https://dentalcraft.app |

Versions are cut by [release-please](https://github.com/googleapis/release-please) from Conventional Commits on `main` (tags `vX.Y.Z`).

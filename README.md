# dentalcraft-app

The shipped DentalCraft desktop application: a Rust workspace with the eframe/wgpu shell, the 3D viewport, the authentication client and the packaging tooling. Depends on `dentalcraft-core` and `dentalcraft-aligners`.

This repository is a submodule of the [`dentalcraft`](https://github.com/dnviti-personal-org/dentalcraft) superproject, which holds the plan, the architecture decisions and the pinned versions of all DentalCraft repositories. Clone the superproject with `--recurse-submodules` to work on the whole project.

- Visibility: private.
- Versioning: [release-please](https://github.com/googleapis/release-please) on `main` (Conventional Commits, tags `vX.Y.Z`).
- CI: GitHub Actions on the `golgari-dev` runner scale set.

## Licence

DentalCraft is proprietary software: copyright (c) 2026 Daniele Viti, all rights reserved. The source code is private. The application is free of charge to download and use, for non-clinical purposes, under the DentalCraft end-user licence agreement in [`LICENSE.md`](LICENSE.md) (Italian and English); it is provided as is, without warranty.

DentalCraft is not a medical device: do not use it on patients or for clinical decisions.

DentalCraft includes open source components, each under its own licence; their copyright and licence notices are in [`THIRD-PARTY-NOTICES.md`](THIRD-PARTY-NOTICES.md). Both texts are built into the programs, so a downloaded copy always carries them:

- in the app: account menu (top right) > **Licence and notices**, or **Terms of use** at the bottom of the sign-in screen;
- on the command line: `dentalcraft-cli licence` prints the licence, `dentalcraft-cli licence --third-party` the third-party notices, and `dentalcraft-cli --version` the copyright line.

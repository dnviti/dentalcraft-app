# dentalcraft-app

The shipped DentalCraft desktop application: a Rust workspace with the eframe/wgpu shell, the 3D viewport, the authentication client and the packaging tooling. Depends on `dentalcraft-core` and `dentalcraft-aligners`.

This repository is a submodule of the [`dentalcraft`](https://github.com/dnviti-personal-org/dentalcraft) superproject, which holds the plan, the architecture decisions and the pinned versions of all DentalCraft repositories. Clone the superproject with `--recurse-submodules` to work on the whole project.

- Visibility: private.
- Versioning: [release-please](https://github.com/googleapis/release-please) on `main` (Conventional Commits, tags `vX.Y.Z`).
- CI: GitHub Actions on the `golgari-dev` runner scale set.

# Notice

Required Notice: Copyright (c) Daniele Viti

## Licence

The source code of dentalcraft-app is source-available under the PolyForm Noncommercial License 1.0.0. The full text is in `LICENSE.md`.

You may use, change and share it for any noncommercial purpose. Commercial use is not allowed. This is not an open source licence in the OSI sense.

## Commercial licensing

For commercial use, contact the author, Daniele Viti: hello@dentalcraft.app.

## Not a medical device

DentalCraft is not a certified medical device. It has no regulatory clearance. Do not use it on patients or for clinical decisions without independent professional validation. It is provided as is, without warranty.

## Third-party components

This repository does not copy third-party source code, apart from the assets listed below. Its Rust dependencies are fetched from crates.io and keep their own licences. They are under MIT, Apache-2.0, or both, plus a few other permissive licences (Unicode-3.0, BSD-2-Clause, BSD-3-Clause, ISC, Zlib, BSL-1.0, CDLA-Permissive-2.0, Unlicense, CC0-1.0, 0BSD), with the exception of one weak-copyleft crate named below.

The complete list, with every licence text, is in `THIRD-PARTY-NOTICES.md`. It is generated for each release from the dependencies of `dc-app` and `dc-cli`, and it is built into the programs (account menu, **Licence and notices**, and `dentalcraft-cli licence --third-party`). The exact versions are in `Cargo.lock`.

### Notable third-party components

- **option-ext** 0.2.0 is under the Mozilla Public License 2.0. It is an unmodified dependency from crates.io. It is not vendored or changed, so the licence notice in `THIRD-PARTY-NOTICES.md` is what MPL-2.0 asks for.
- **self_cell** is `Apache-2.0 OR GPL-2.0-only` and is used under Apache-2.0. A few other crates offer a permissive option next to an LGPL one and are used under the permissive one.
- **epaint_default_fonts** (pulled in by egui) is `(MIT OR Apache-2.0) AND OFL-1.1 AND Ubuntu-font-1.0`: MIT or Apache-2.0 code, with fonts under the SIL Open Font License 1.1 and the Ubuntu Font Licence 1.0.
- **ring** is under `Apache-2.0 AND ISC`, **webpki-roots** under CDLA-Permissive-2.0, **clipboard-win** and **error-code** under BSL-1.0.

### Bundled fonts and icons

- **Phosphor Icons** (`crates/dc-ui/assets/fonts/phosphor/Phosphor.ttf`): MIT. Licence text: `LICENSE-Phosphor-MIT.txt`.
- **Icon codepoints** (`crates/dc-ui/src/phosphor_regular.rs`), vendored from egui-phosphor 0.14.0: MIT OR Apache-2.0. Licence text: `LICENSE-egui-phosphor-MIT.txt`.
- **Atkinson Hyperlegible Next** (five weights, `crates/dc-ui/assets/fonts/atkinson-hyperlegible-next/`): SIL Open Font License 1.1. Licence text: `OFL.txt` in the same folder.
- **B612 Mono** (Regular and Bold, `crates/dc-ui/assets/fonts/b612-mono/`): SIL Open Font License 1.1. Licence text: `OFL.txt` in the same folder.

### Release signing keys

`packaging/keys/`, `packaging/windows/dentalcraft-selfsigned.crt.pem` and `packaging/windows/timestamp-root.pem` are public keys and certificates, kept here so that installers and updates can be checked. No private key is in this repository.

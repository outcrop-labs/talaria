- **The omapak Flatpak opened on "Could not connect to localhost: Connection
  refused".** Tauri treats the *absence* of the `custom-protocol` Cargo
  feature as `cfg(dev)` even for `--release`: `generate_context!` skips
  `frontendDist` and the window loads `tauri.conf.json`'s `devUrl`
  (`http://localhost:5290`). `tauri build` (GitHub Release installers) adds
  the feature; omapak's source build is a plain `cargo build --release` after
  `bun run build:vite`, so the published `app.talaria.desktop` was a Vite
  client with nothing listening. The crate now defines `custom-protocol`
  (`tauri/custom-protocol`); packagers that skip the CLI pass
  `--features custom-protocol`. It is not default: `generate_context!`
  panics when `frontendDist` is missing, and clippy/tests have no
  `desktop/dist`. Verified: `cargo metadata` lists the feature;
  `cargo tree -e features` without the flag does not enable
  `tauri/custom-protocol`; `bun run check`.

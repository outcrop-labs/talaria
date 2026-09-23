- **Talaria Desktop ships installers for all three desktops, built and
  attached by CI (`.github/workflows/desktop-package.yml`,
  [`docs/DESKTOP.md`](./docs/DESKTOP.md)).** Tauri's bundlers cover linux
  deb/rpm/AppImage, macOS as one universal build, and windows nsis (plus an
  `.msi` when the version has no pre-release part — Windows product versions
  have a numeric fourth field, so a `v0.2.0-rc.1` release ships the `.exe`
  rather than an `.msi` claiming to be `0.2.0`); the two targets Tauri has none
  of — Arch's pacman and flatpak — are built from one staged FHS tree in
  `desktop/packaging/` (`stage.sh`, then `pack-pacman.sh`, or
  `flatpak-builder` against the GNOME 50 runtime). The bundle targets moved
  out of the previously-empty `targets` list into per-platform config files
  (`tauri.linux.conf.json`, `tauri.windows.conf.json`,
  `tauri.macos.conf.json`), so `tauri build` produces that platform's
  installers instead of a bare binary — and the released version is merged in
  at build time (`--config`), because the tag is the version authority
  (RELEASING.md) and nothing in the repo carries a released version. On a
  `vX.Y.Z[-rc.N]` tag, `release.yml` calls the workflow once the image is
  pushed and every installer lands on that GitHub Release with a
  `SHA256SUMS`; a push to main touching `desktop/` leaves the same set as
  workflow artifacts. Nothing is signed or notarized — no Apple Developer
  certificate and no Windows signing key exist here — so macOS wants a
  right-click → Open the first time and Windows warns through SmartScreen.
  Verified: `bun run desktop:check` green (fmt, clippy, 14 tests,
  svelte-check); a local `tauri build` produced the release binary and a
  `.deb` stamped with the injected version (`Version: 0.0.0-test`,
  `Depends: libwebkit2gtk-4.1-0, libgtk-3-0`); `packaging/linux/*.sh` turned
  that binary into a `.pkg.tar.zst` that `pacman -Qip` reads back
  (`talaria-desktop-bin 0.2.0_rc.1-1`, deps `webkit2gtk-4.1`/`gtk3`, x86_64).
  The full matrix then ran in CI
  ([run](https://github.com/outcrop-labs/talaria/actions/runs/35064711777)):
  six jobs green, producing
  `Talaria_<v>_amd64.{deb,AppImage}`, `Talaria-<v>-1.x86_64.rpm`,
  `Talaria-<v>-x86_64.pkg.tar.zst`, `Talaria-<v>-x86_64.flatpak`,
  `Talaria_<v>_universal.dmg` + `.app.zip`, and the NSIS `.exe` — whose
  control metadata, `pacman -Qip` output, DMG/PE headers and the AppImage's
  ELF were all read back afterwards. Two of those runs earned their keep:
  flatpak-builder needs `eu-strip` (elfutils), and the msi bundler refuses a
  pre-release version, hence the conditional `.msi`. A third did: the
  CI-built AppImage aborted on Arch (`Could not create surfaceless EGL
  display: EGL_BAD_ALLOC`) — bisected to linuxdeploy's bundled
  `libwayland-client.so.0`, excluded by a second repack pass
  (`packaging/linux/repack-appimage.sh`) — and the repacked artifact was then
  run on that same host: window up, the registered instance loaded.
  Each platform's own job then proves the artifact comes up, not merely that
  it built
  ([run](https://github.com/outcrop-labs/talaria/actions/runs/35098785546)):
  macOS verifies the dmg (`disk image (Apple_HFS : 4): verified`) and launches
  the .app (still running 20 s later), windows installs the installer it just
  produced and launches what it installed
  (`C:\Users\…\AppData\Local\Talaria\talaria-desktop.exe`, still running), and
  the deb, rpm, pacman and flatpak packages were each installed in a clean
  container of their own distribution (Ubuntu, Fedora, Arch). What stays
  unverified is what no runner can be: Gatekeeper and SmartScreen as a user
  meets them — an artifact built on a runner was never quarantined — and
  session isolation on macOS/Windows, which `data_directory` does not provide.

### Changed

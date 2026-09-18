# Talaria Desktop

The desktop app is a **multitenant shell** around Talaria instances — the connect-out half of
the roadmap's business-multitenancy line ("swap between them in a click … connect out to other
hosted Talaria instances"). It is deliberately NOT a second UI codebase: the interior is always
the real web UI, loaded live from each instance's origin. The only local frontend is the
launcher's welcome screen.

Built on [Tauri v2](https://v2.tauri.app/) (Rust shell + WebKitGTK webviews on Linux), tracked
in [`desktop/`](../desktop).

## How it works

Two views, one window:

- **welcome** — the launcher (webview label `main`, local content from `desktop/src`) fills the
  window: the wing mark on the signature dither field, the instance list, add. The brand
  treatment is ported, not imitated — `WingMark.svelte`, `DitherLayer.svelte` + `lib/dither.ts`
  are verbatim from ui/ (keep them in step), and typography is Mercury's IBM Plex pair.
- **active** — the instance's webview IS the window; the launcher is hidden (but alive).
  Switching happens **inside the instance UI**: the desktop switcher
  (`ui/src/components/app/DesktopSwitcher.svelte`) wears the current instance's identity
  (avatar chip + label) beside the logo in the nav rail — and in the login screen's top-left
  corner, so a signed-out instance is not a dead end. It renders ONLY inside the shell: the
  guard is `inDesktopShell()` from `ui/src/lib/desktop-shell.ts` (a function, evaluated at
  render — module-load order must never decide it), and a browser gets nothing. `Ctrl/Cmd+
  Shift+H` (an initialization script the shell injects into every instance webview) is the
  floor: it opens the launcher even on instances whose Talaria version predates the switcher.

Each instance webview gets its own `data_directory`, so cookie jars and localStorage never mix
— cookies ignore ports, so without this, two dev instances on `127.0.0.1:530x` would silently
share a session. Hidden webviews stay loaded: sessions and SSE survive a switch.

Adding an instance: the shell normalizes the URL (bare hosts become `https://`, only http(s),
credentials/paths stripped — an instance IS its origin), then probes the instance beacon
(`GET /api/well-known/talaria-instance` → `{instance, companyName}`). A valid beacon uuid is
what makes a URL "a Talaria instance"; the uuid is also the dedupe key. The label comes from
`companyName`, falling back to host(:port).

The registry is `~/.local/share/app.talaria.desktop/instances.json` (atomic writes), and each
instance's session data lives beside it in `instances/<id>/webview/`. Removing an instance
deletes its data dir — the saved session dies with it; the instance itself is untouched.

## Security posture

- **Instance webviews get the switcher plus window chrome, nothing else.** The shell's
  commands are gated behind capabilities (`AppManifest::commands` in `desktop/src-tauri/build.rs`);
  the launcher holds all of them (`capabilities/launcher.json`), and each REGISTERED instance
  origin gets a runtime grant (`grant_switcher` in `src-tauri/src/lib.rs`, via Tauri's
  dynamic-ACL) scoped to `list_instances`, `activate_instance`, `show_welcome`,
  `get_desktop_settings`, `set_titlebar_mode`, `desktop_window` on that one webview. No fs,
  no generic window API — remote content can switch instances and move/close *this* window.
  An origin the user never registered gets no IPC at all.
- Login happens inside each instance webview via the instance's own flows (password or
  OAuth). **Known limitation:** Google blocks OAuth inside embedded webviews; password login
  works, and for dev instances a session can be minted into Redis directly (see the
  `repo-traps` skill).
- The shell never holds instance credentials; it holds URLs.

## WebKitGTK geometry (the fragile part)

Multiwebview-per-window rides Tauri's `unstable` cargo feature, with known Linux bugs:
children can be misplaced ([tauri#10420]) and `auto_resize` breaks across resize cycles
([tauri#10131]) — and in practice the two webviews have even been observed STACKING
VERTICALLY in the GTK container. The design leans on none of it: exactly ONE webview is
visible at a time (`hide()`/`show()` decide who renders; bounds calls are belt-and-suspenders
in logical units, recomputed on every `WindowEvent::Resized`), so there is no side-by-side
layout to get wrong. `tauri` is exact-pinned (`=2.11.5`) in `desktop/src-tauri/Cargo.toml` —
unstable features are semver-exempt, so upgrades are deliberate.

[tauri#10420]: https://github.com/tauri-apps/tauri/issues/10420
[tauri#10131]: https://github.com/tauri-apps/tauri/issues/10131

## WebKit, not Chrome

The engine is WebKitGTK, so the instance UI runs under WebKit (Safari-family) semantics, not
Blink. APIs Chrome has that WebKit lacks (e.g. `requestIdleCallback`) are landmines in ui/
code even for browser Safari — feature-detect with `typeof x === 'function'`, never a bare
reference (a bare global throws ReferenceError before `??` can fall back).

## Development: the box builds, the host runs

A devbox has no display, so the split is: **code and gates in the box, GUI on the host** (the
box's clone is host-visible at `~/Development/devboxes/<name>/talaria`).

```sh
# host, once: the webkit runtime (Arch; gtk3 rides along)
sudo pacman -S webkit2gtk-4.1 librsvg

# host: image (carries the tauri build deps) and the box
bun talaria box build
bun talaria box new desktop

# in the box — the gates (CARGO_TARGET_DIR keeps cargo artifacts OFF the
# host-shared tree; the host's build uses desktop/src-tauri/target, and two
# cargo builds into one target dir fight)
bun talaria box enter desktop
CARGO_TARGET_DIR=$HOME/target-desktop bun run desktop:check

# in the box — a live instance to point the shell at (seeded at box creation)
bun talaria dev          # → http://127.0.0.1:5302

# on the host — the GUI (vite for the launcher, then the binary)
cd ~/Development/devboxes/desktop/talaria/desktop
bun install              # shares node_modules with the box: same bun, same platform
bun run dev:vite &       # the launcher's dev server on :5290
./src-tauri/target/debug/talaria-desktop
```

In the app: *Add instance* → `http://127.0.0.1:5302`. The switcher appears the moment that
instance's UI loads (login screen included). Prove isolation with a second instance on the
same host at a different port (another box, or the primary stack) — log into both and swap;
both sessions survive.

One dev-loop trap: `tauri dev`'s watcher kills and relaunches the app on source/config
change, and on WebKitGTK that kill has been observed to corrupt the relaunched process
(silent death, sometimes `free(): corrupted unsorted chunks`). If the window doesn't come
back after a rebuild, stop the dev process and start it again — or, with vite still up, run
the binary directly as above.

## Gates

`bun run desktop:check` (root) = `cargo fmt --check` + `cargo clippy -D warnings` +
`cargo test` + `svelte-check`, the same list CI's `desktop` job runs. `bun run verify` is
unchanged — the desktop surface gates per-surface, exactly like `api:check`.

## Installers

`.github/workflows/desktop-package.yml` builds every installer and attaches it to the
GitHub Release a version tag opens — `release.yml` calls it after the image push
(RELEASING.md has the operator's half). A push to main that touches `desktop/` builds the
same set and leaves it as workflow artifacts.

| Platform | Files |
|---|---|
| Linux | `.deb`, `.rpm`, `AppImage` from Tauri's bundlers; `.pkg.tar.zst` (pacman) and `.flatpak` from `packaging/` |
| macOS | universal `.dmg` (Apple silicon and Intel) plus the `.app` as a zip |
| Windows | NSIS `.exe`, plus an `.msi` when the version has no pre-release part |

Bundle targets live per platform — `tauri.linux.conf.json`, `tauri.windows.conf.json`,
`tauri.macos.conf.json`, which Tauri merges over the base config automatically — so
`bun run build` in `desktop/` produces that platform's installers and nothing else. The
released version is merged in at build time with `tauri build --config`; no file in the
repo carries it, the same way tags are the version authority everywhere else.

Tauri has no pacman or flatpak target, so `desktop/packaging/` builds both from ONE staged
FHS tree: `linux/stage.sh` writes it, `linux/pack-pacman.sh` turns it into a package
(`tar` + `.PKGINFO` — no `makepkg`, there is nothing to compile), and `flatpak/build.sh`
builds the manifest next to it against the GNOME runtime. Building those two locally needs
`zstd` and `tar` for pacman, and `flatpak` + `flatpak-builder` + the runtime named in the
manifest for the flatpak.

A source build that does not go through `tauri build` (omapack, a packager
that runs `cargo build --release` after `bun run build:vite`) still has to
enable Tauri's `custom-protocol` feature. Without it the binary is `cfg(dev)`
even in release and the webview loads `http://localhost:5290`. That feature is
default-on in `desktop/src-tauri/Cargo.toml` for exactly this; `tauri dev`
strips it. `stage.sh` also installs AppStream metainfo
(`app.talaria.desktop.metainfo.xml`) next to the desktop entry.

Nothing is signed or notarized: there is no Apple Developer certificate and no Windows
signing key in this repo, so macOS wants a right-click → Open the first time (Gatekeeper)
and Windows shows a SmartScreen warning. The assets carry a `SHA256SUMS`, and the honest
account of where an installer came from is the workflow run that built it.

Two packaging details worth knowing before editing those configs. The deb's `Depends` comes
from Tauri's own defaults for the crates the shell links (`libwebkit2gtk-4.1-0`,
`libgtk-3-0`); a list in `tauri.linux.conf.json` would only duplicate them, so the workflow
asserts the built deb declares them instead. rpm has no defaults at all, which is why its
dependencies are written out and the deb's are not.

And the version is not free-form across the bundlers: Windows product versioning has a
numeric fourth field, so an `.msi` can only be built from a version with no pre-release
part. The workflow therefore bundles NSIS always and adds the msi only for a stable
version — a `v0.2.0-rc.1` release ships the `.exe` and no `.msi`, rather than an `.msi`
that claims to be `0.2.0`. Everything else takes the tag's version verbatim
(`Talaria_0.2.0-rc.1_amd64.deb`, `Talaria-0.2.0-rc.1-1.x86_64.rpm`, the dmg, the flatpak
bundle name).

Separately, a local AppImage build does not work on Arch — linuxdeploy's bundled `strip`
cannot read Arch's `.relr.dyn` sections, and Tauri's gtk plugin looks for gdk-pixbuf at a
Debian path — which is a limitation of that toolchain on that host, not of this config: the
AppImage is built on the Ubuntu runner. A pacman package can be built and checked locally
on Arch (`stage.sh` → `pack-pacman.sh` → `pacman -Qip`), and the flatpak needs `flatpak`,
`flatpak-builder` and `elfutils` (flatpak-builder's `eu-strip`) plus the runtime named in
the manifest.

The AppImage takes one extra pass for the same reason the other formats do not:
linuxdeploy's gtk plugin deploys the build distro's `libwayland-client.so.0`, a library
whose job is to talk to the *local* compositor and driver, and a bundle carrying a foreign
one aborts before a window exists on a host with a different Wayland stack (Arch:
`Could not create surfaceless EGL display: EGL_BAD_ALLOC`). The CLI has no way to ask for
its exclusion — the config can only add files — so `packaging/linux/repack-appimage.sh`
trims the AppDir and runs linuxdeploy once more with `--exclude-library`, on the same
runner that built it (dependency resolution is host-based; a pass on another distro
re-deploys that distro's libraries).

Until a platform is exercised, "the installer built" and "the installer works" are
different claims, so the pipeline makes the runners prove what they can: macOS mounts the
dmg and launches the app, windows installs whichever installer the run produced and
launches what it installed, and each job fails if the app is not still running twenty
seconds later. When this pipeline was written, the Linux formats were installed in clean
containers of their own distributions (Ubuntu, Fedora, Arch) and the AppImage was run on an
Arch host, which is a one-off rather than something CI repeats.

What no runner here can test: Gatekeeper and SmartScreen as a user meets them — an artifact
built on a runner was never quarantined, and nothing here is signed — and the session
isolation that `data_directory` does not provide on macOS or Windows.

## Window chrome

The window ships `decorations: false`. Three titlebar modes live in
`~/.local/share/app.talaria.desktop/settings.json` (themed / os / none). **Themed is the
default on every OS**: custom min/max/close drawn in the webview (traffic-light side follows
the OS — left on macOS, right elsewhere) and a bar you can drag. OS turns native decorations
on. None hides both. Settings → Profile (inside the desktop app) and the launcher welcome
screen both set it.

## Updates

A stable GitHub Release carries `latest.json` plus minisign signatures of the AppImage,
the universal `.app.tar.gz`, and the NSIS installer. The running app checks
`/releases/latest/download/latest.json` (Settings → Profile, or the launcher), verifies
the payload against the pubkey in `tauri.conf.json`, replaces itself, and relaunches.
RCs do not publish `latest.json` — `/releases/latest` is GitHub's stable pointer.

Signing uses `TAURI_SIGNING_PRIVATE_KEY` (a GitHub Actions secret). Losing that key
means installed copies can no longer verify a new payload. OS code signing
(Gatekeeper / SmartScreen) is still absent; this signature is the updater's own.

## Deferred on purpose

Rename/reorder instances, health badges, native notifications, tray, deep links, OAuth via
external browser, code signing and notarization.

macOS and Windows build and ship, but the session-isolation invariant above does not hold
there yet: `data_directory` is a no-op on those platforms (macOS wants
`data_store_identifier`), so two instances served from one host:port would share a cookie
jar. Distinct origins — every hosted instance — are unaffected.

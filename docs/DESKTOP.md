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
  window: the wing mark on the signature dither field, the instance list, add. The dither
  material is not copied — `desktop/vite.config.ts` aliases `@dither` onto ui's
  `dither-engine.ts` (which imports nothing, so it is safe to share) — but the launcher does
  not run the live engine. `desktop/src/lib/dither-field.ts` paints the same field (ui's
  `evalSource`, Bayer matrix, two-tier rule, clump hash and page lattice) ONCE, as pixels
  through one `putImageData`, and `DitherField.svelte` repaints it only after its box or the
  device pixel ratio has stopped changing. "The launcher's cost" below says why.
  Typography is Mercury's IBM Plex pair, shipped inside the app (`@fontsource`, Latin and
  Latin Extended, the two weights the launcher sets): the launcher makes no network request
  before its first paint.
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

The probe has two bounds (`desktop/src-tauri/src/beacon.rs`): 5 s to connect, so a
black-holed address fails in seconds, and 20 s in total, so a slow but live instance is
added rather than refused. A failure is classified from the error's source chain — refused,
does not resolve, timed out, unreachable, untrusted certificate, TLS failed — instead of
reqwest's one sentence for all of them, and a TLS failure on a bare host suggests the
`http://` address. Cancel and Escape work while the probe runs: `cancel_add_instance` aborts
it, and a cancelled add never reaches the registry, even if the instance answers a moment
later.

The registry is `~/.local/share/app.talaria.desktop/instances.json` (atomic writes), and each
instance's session data lives beside it in `instances/<id>/webview/`. Removing an instance
deletes its data dir — the saved session dies with it; the instance itself is untouched. The
closed webview's WebKit network process can recreate its storage dir while it shuts down, so
the remove deletes again 2 s and 10 s later. The registry also records the removal
(`removed`), and each launch deletes a recorded dir that came back; a removal is dropped
once a launch finds its dir gone. Nothing else under `instances/` is ever deleted: a dir no
entry owns and no remove recorded may be the session of an entry a damaged registry lost.

Neither file can stop the app from starting. A registry or `settings.json` that does not
parse is moved aside next to itself (`instances.json.corrupt-<ms>`), whatever entries still
parse are kept, and the launcher says what happened. The clean file written in its place
records no removals, so no later launch deletes a data dir on the damaged file's word. A
settings value this build does not know (a newer build wrote it) reads as the default. The
registry also records `lastView`, the instance the window last showed or `null` for the
launcher, so a relaunch lands where the person left. A registry from before that field
reopens the most recently opened instance, and only one that was actually opened.

### Opening an instance, and when it does not load

An instance with no webview yet (the first open of a session, a relaunch) is probed first,
from the launcher, which shows "Opening X…" with a way to stay. An unreachable instance is
then a sentence with Retry on the launcher, not a blank window or WebKit's error page — in
the themed titlebar mode the only chrome is drawn by the page, so that page was a trap.
The card also offers **Open anyway**, because the check is not the webview: it is the
shell's own HTTP client, with no cookies and its own proxy lookup and TLS trust (on Linux,
the proxy comes only from the environment). An instance behind a sign-in gate that refuses
the beacon without a session, or one reachable only through a proxy set in the desktop's
settings, fails the check and still loads in the webview. Open anyway opens it without the
check; on Linux a page that then fails comes back as a load failure (below), and on macOS
and Windows the person gets what the platform webview shows for a failed load, as every open
did before the probe.

**Linux only:** each instance webview's WebKitGTK `load-failed` signal is watched
(`commands.rs::watch_load_failures`; the `webkit2gtk` crate is a Linux-only dependency). An
http(s) main-frame load that fails — the instance went away mid-session, a full reload
while it is down — closes that webview and returns to the launcher with WebKit's reason and
Retry. A navigation that was replaced, a download, a link WebKit hands elsewhere (`mailto:`)
or content it cannot show inline is answered "handled", so the page the person is on stays;
left to WebKit, a `mailto:` link replaced the whole instance with "The URL can't be shown".
macOS and Windows rely on the probe before a cold open.

### The launcher follows the shell

The launcher mirrors Rust's state rather than reading it once: Rust emits `shell-state` to
the launcher webview on every change (view, active instance, titlebar, the registry, load
failures), and the launcher re-reads it whenever it becomes visible. An instance's switcher,
its Settings → Profile titlebar choice and a notification click all change that state while
the launcher is hidden. Inside instances, ui's `desktop-settings.svelte.ts` re-reads the
titlebar mode each time its webview is shown, for the same reason: the window is shared, and
a choice made in one instance reaches the others.

## Security posture

- **Instance webviews get the switcher, window chrome, and a notification post — nothing else.** The shell's
  commands are gated behind capabilities (`AppManifest::commands` in `desktop/src-tauri/build.rs`);
  the launcher holds all of them (`capabilities/launcher.json`), and each REGISTERED instance
  origin gets a runtime grant (`grant_switcher` in `src-tauri/src/lib.rs`, via Tauri's
  dynamic-ACL) scoped to `list_instances`, `activate_instance`, `show_welcome`,
  `get_desktop_settings`, `set_titlebar_mode`, `desktop_window`, `check_for_update`,
  `install_update`, and `desktop_notify` on that one webview. No fs, no generic window API —
  remote content can switch instances, move/close *this* window, and post a notification
  (title, body, and a same-origin path). An origin the user never registered gets no IPC at all.
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
in logical units, applied once at the end of setup and again on every `WindowEvent::Resized`),
so there is no side-by-side layout to get wrong. Visibility changes hide everything else FIRST,
then show the one webview. And the shell never calls `Window::show()` on a window that is
already visible: on Linux tao's show is gtk `show_all`, which re-shows every hidden child
webview, and the vertical GtkBox then split the window between them (a notification click
did exactly this). The setup pass matters: the first resize can
arrive before shell state exists, and a missed `set_bounds` leaves the launcher webview at
WebKitGTK's default 1×1. An undecorated window that never paints is not mapped on Wayland —
the process is alive and the launch looks silent. `tauri` is exact-pinned (`=2.11.5`) in
`desktop/src-tauri/Cargo.toml` —
unstable features are semver-exempt, so upgrades are deliberate.

[tauri#10420]: https://github.com/tauri-apps/tauri/issues/10420
[tauri#10131]: https://github.com/tauri-apps/tauri/issues/10131

## WebKit, not Chrome

The engine is WebKitGTK, so the instance UI runs under WebKit (Safari-family) semantics, not
Blink. APIs Chrome has that WebKit lacks (e.g. `requestIdleCallback`) are landmines in ui/
code even for browser Safari — feature-detect with `typeof x === 'function'`, never a bare
reference (a bare global throws ReferenceError before `??` can fall back).

## The launcher's cost

Measured on a headless WebKitGTK 2.52 with no GPU (Mesa llvmpipe), 2026-10-28; the numbers
and the rig are in the PR that made these changes. Three things made the launcher slow, and
each has a rule now:

- **No live canvas field.** ui's live engine over the whole window, with a shimmer and a
  uniform grain, re-rolled about half of ~246 000 cells every 160 ms as individual 1-px
  `clearRect` + `fillRect` pairs. WebKit's web process main thread sat at 97 %, a frame took
  ~0.5 s, and every click, keystroke and IPC reply waited behind it (opening the add dialog
  took 0.54–0.61 s to show). The static field idles at ~0 % and the same click shows in
  ~80 ms. WebKit's renderer switches (`WEBKIT_DISABLE_COMPOSITING_MODE`,
  `WEBKIT_DISABLE_DMABUF_RENDERER`, Skia CPU) changed nothing, so no env default is set.
- **No overlay scrollbars.** WebKitGTK's overlay scrollbar fades in and out on every scroll,
  and on a software renderer that fade composited the whole window at full rate for ~3 s —
  300–400 % CPU after each scroll of a long instance list. The launcher styles
  `::-webkit-scrollbar`, which has no fade.
- **Nothing that waits on another process runs on the main thread.** A sync command runs on
  the main thread in Tauri v2, which is also the GTK loop every webview paints from.
  `desktop_notify` was sync, and a notification server that took 30 s to answer froze the
  whole app for 30 s; it is async now, the D-Bus post runs on a blocking thread, and the page
  gets its answer within 2 s.

The window and every instance webview paint the launcher's ground (`#090a09`) until their
page does, instead of GTK's grey and WebKit's white.

Each OPENED instance keeps its webview, and with it a WebKit web process and, because of its
own data directory, its own network process: about 110 MB + 13 MB for a trivial page, ~690
MB for the real UI (2 web processes). 25 opened mock instances were 3.07 GB. Nothing is
discarded, by design (sessions and SSE survive a switch); a cold open of an instance is
~0.7 s of WebKit starting those two processes.

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
pass `--features custom-protocol`. Without it the binary is `cfg(dev)` even
in release and the webview loads `http://localhost:5290`. The feature is
defined in `desktop/src-tauri/Cargo.toml` and is not default: clippy/tests
have no `desktop/dist`, and `generate_context!` panics if that path is
missing. `stage.sh` also installs AppStream metainfo
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
screen both set it. Turning the OS titlebar on keeps the window's outer size (the content
loses the titlebar's height), and the shell re-places the visible webview straight away
rather than a frame later, which used to flash a band of bare window.

The window opens at 1280×800, shrunk to fit the monitor it opens on when that is smaller
(`layout::fit`: a 1440×900 panel at 167 % is 864×540 logical, and the fixed size put the
themed min/max/close off-screen). The minimum is 760×480, also shrunk to fit.

## Updates

A stable GitHub Release carries `latest.json` plus minisign signatures of the AppImage,
the universal `.app.tar.gz`, and the NSIS installer. The running app checks
`/releases/latest/download/latest.json` (Settings → Profile, or the launcher), verifies
the payload against the pubkey in `tauri.conf.json`, replaces itself, and relaunches. The
check is bounded at 10 s (the plugin's default is no bound); the install is not, because the
plugin applies one timeout to the check and to the whole download.
RCs do not publish `latest.json` — `/releases/latest` is GitHub's stable pointer.

Signing uses `TAURI_SIGNING_PRIVATE_KEY` (a GitHub Actions secret). Losing that key
means installed copies can no longer verify a new payload. OS code signing
(Gatekeeper / SmartScreen) is still absent; this signature is the updater's own.

## Notifications

The instance UI's Desktop notifications toggle posts through the shell (`desktop_notify`),
not the webview's Notification API. WKWebView and WebKitGTK report that API denied, and
there is no site-settings page inside the shell, so a page cannot grant it. The shell posts
through the OS notification service — the same path other desktop apps use. A click focuses
this window, brings that instance forward, and opens the notification's path when it is a
same-origin path. The app has to be running; a quit process is not listening. Tray and
delivery after quit stay deferred. The post is off the main thread (see "The launcher's
cost"), and the click never calls `show()` on the already-visible window (see the geometry
section).

## Deferred on purpose

Rename/reorder instances, health badges, tray, deep links, OAuth via
external browser, code signing and notarization.

macOS and Windows build and ship, but the session-isolation invariant above does not hold
there yet: `data_directory` is a no-op on those platforms (macOS wants
`data_store_identifier`), so two instances served from one host:port would share a cookie
jar. Distinct origins — every hosted instance — are unaffected.

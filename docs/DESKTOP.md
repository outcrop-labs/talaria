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
share a session. Hidden webviews stay loaded, so sessions and SSE survive a switch, until an
instance has been off screen for the suspend timeout ("Suspending inactive instances" below).

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
reopens the most recently opened instance, and only one that was actually opened. It also
keeps each instance's last page (`lastUrls`: path, query and hash), so a relaunch, or any
open of an instance with no webview, returns to that page instead of the root. The pages
are kept beside the entries, not in them: every other instance's in-app switcher reads the
entries (`list_instances`), and one instance's paths are no business of another origin. A
sign-in or callback page is never kept (`suspend::restorable_url`: `/login`, `/claim`,
`/join`, `/auth`, `/oauth`, any `/api/` URL, any path with `callback` in it, any URL with a
one-time value such as `code`, `state` or a `token` in its query or fragment); the instance
root is used instead. A page on another origin (a link that left the instance) leaves the
last page of the instance itself standing. Older builds ignore `lastUrls` and drop it on
save.

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

- **Instance webviews get the switcher, window chrome, the notification commands and the
  suspend report — nothing else.** The shell's
  commands are gated behind capabilities (`AppManifest::commands` in `desktop/src-tauri/build.rs`);
  the launcher holds all of them (`capabilities/launcher.json`), and each REGISTERED instance
  origin gets a runtime grant (`grant_switcher` in `src-tauri/src/lib.rs`, via Tauri's
  dynamic-ACL) of `INSTANCE_PERMISSIONS` on that one webview: `list_instances`,
  `activate_instance`, `show_welcome`, `get_desktop_settings`, `set_titlebar_mode`,
  `set_suspend_after`, `desktop_window`, `check_for_update`, `install_update`,
  `desktop_notify`, `desktop_notification_status`, `desktop_notification_request`,
  `desktop_notification_test`, `desktop_set_badge`, `desktop_quit`,
  `set_close_to_background` and `desktop_suspend_report`. A suspend report counts only as
  the answer to the question the shell just asked that webview (a nonce). No fs, no generic
  window API, and **no `core:*` permission** (a unit test holds the line): `core:event`
  would let a page listen, and an event emitted to `EventTarget::Any` reaches every
  listener whatever its webview, so one instance could hear another's. A notification
  click reaches its page by `eval` of a validated script instead (see "Notifications").
  Remote content can switch instances, move/close *this* window, quit, and post a
  notification — a title, a body, a tag, a same-origin path and a kind, clamped, at most a
  burst of 20 and then one every 2 s per webview (`rate limited` past it); a test at most
  every 5 s and a permission request every 10 s. Its badge is its own share of the count.
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
MB for the real UI (2 web processes). 25 opened mock instances were 3.07 GB on the
2026-10-28 rig (4.35 GB PSS on the 2026-10-29 one). An instance that stays off screen is
suspended, which frees its web process: the next section.

## Suspending inactive instances

`desktop/src-tauri/src/suspend.rs` (the clock, the URL rule and the scripts), `commands.rs`
(the wiring) and `watch.rs` (notifications while suspended).

**When.** An instance whose page has not been on screen for the timeout is suspended: its
webview is closed. "Not on screen" means another instance or the launcher has been showing.
The instance the window shows counts only while the window itself is minimized or hidden,
never while it is merely unfocused, so a page on a second monitor (an agent run streaming)
stays. GTK on Wayland reports no minimized state, so there the shell counts its own minimize
(the themed titlebar's button) until the window is focused again; on Wayland a window
minimized by the compositor or by the OS titlebar's button is not seen as away, and its
instance stays loaded. A compositor may also ignore the shell's minimize (sway and other
wlroots window managers): the shell looks once, 750 ms after asking, and a window still
focused then is counted as on screen. One that ignored it but lost focus within that moment
counts as away until it is focused again. X11, macOS and Windows report minimized
themselves. The timeout is
`suspendAfterMinutes` in `settings.json`: 10 by default, `0` for Off, any whole number of
minutes up to a day. The launcher and Settings → Profile offer Off, 5, 10, 30 and 60
minutes; a value set by hand shows as its own choice. The clock ticks every 5 s.

**Only a page that agrees.** Before it closes one, the shell asks the page
(`suspend::report_js`) through ui's `window.__TALARIA_DESKTOP__` hook
(`ui/src/lib/desktop-suspend.ts`). The page stays open, until it is next shown, when:

- it has no hook: an instance whose Talaria predates it. Its notifications and its typed
  input are its own page's business, so it behaves exactly as before;
- it holds input a reload would lose:
  - a field the person changed that is still on the page and differs from what it held
    before their first change: text in an input, a textarea or a rich editor, a select
    picked, a checkbox or radio toggled. ui keeps the chat composer's draft across a reload
    (`lib/comms-drafts`; the editor is marked `data-desktop-draft`), so the composer's text
    does not count. A half-written ticket field, an agent prompt mid-edit, a typed search
    or an unsaved form's select does;
  - anything marked `data-desktop-unsaved`: the pending-attachment chips over a composer
    (uploaded, not yet sent, and not part of the draft). A file input is not counted
    itself: every one in ui hands its file off on pick, as such a chip or a saved upload;
  - not seen: input inside an embedded frame. The artifact preview is a sandboxed frame
    with its own origin and a Google file pane is Google's, so the page cannot read either;
- its page posts OS notifications and the shell cannot read them the way the page does
  (below).

A page that says it can go is closed once the shell has read its cookies and its
notifications baseline, which can take a while on a slow instance. It is not asked again
meanwhile, so one page never starts two closes or two watchers. Both reads are bounded (10 s
for the cookie store, 30 s for the baseline), and a page the shell could not read in time
stays.

**What a restore brings back.** Showing a suspended instance by any route recreates its
webview at its exact last page (path, query and hash), at once and with no probe: a launcher
row, the in-app switcher, `Ctrl/Cmd+Shift+H` then a row, and the window restored or focused
while that instance is the one it shows. A relaunch with it as `lastView` opens it at the
same page (a cold open, so probed first, as before). A notification click opens the
notification's own path instead. The recreated page also gets back its `sessionStorage`
(where ui keeps each view's "where you were") and the scroll position of every box that was
scrolled, retried while the page renders for up to five seconds and abandoned at the
person's first scroll, key or click. Not restored: in-memory state a reload also loses (an
open menu or dialog), input the page did not keep (such a page is never suspended),
and scroll after a relaunch (only the URL is kept on disk).

**What survives.**

- *The session.* ui's session cookie is persistent (`Max-Age` 7 days), so it is on disk in
  the instance's data directory and the restored page is still signed in.
- *Notifications.* A suspended page posts nothing, so the shell does its job: `watch.rs`
  reads `/api/notifications` with that instance's own cookies (taken from its webview's
  store, sent only to its origin, redirects not followed), made live by `/api/me/events`
  with the page's 30 s poll floor. The first read is taken while the page is still open: if
  it fails for anything but a 401/403 (a proxy or certificate only the webview knows, the
  instance down), the page is kept. It runs only when the page said it would post (ui's
  `browserNotifyEnabled`), and keeps the page's own arrival rule (interface I-3): a row is
  posted when it is unread, newer than the high-water mark less two minutes, and unseen —
  or seen at an earlier `createdAt`, which is a re-alert under the same tag. **It starts
  from the page's record**, `localStorage['talaria.notify.arrivals.v1']`, which the suspend
  report hands over (`arrivals`): whatever the page missed — a dead stream, a sleep — is
  posted on the watcher's first read instead of being swallowed into a new baseline (D2).
  A page with no record (an older UI) gets the old baseline. More than three at once are
  one "N new in <instance>" banner. A failed stream — a 502 while the instance restarts, a
  deploy, Cloudflare dropping an idle connection — is reopened with backoff (1 s doubling to
  30 s, reset after a minute up), every reopen reads the rows again, and the 30 s poll floor
  keeps reading through the outage; a 401/403 from the instance itself (its JSON) ends the
  watcher, while a Cloudflare challenge (`cf-mitigated`, an HTML 403) counts as an outage —
  the shell's requests also name themselves (`User-Agent: Talaria-Desktop`), which a bot
  filter wants. Each read sets the
  instance's share of the badge from the server's `unread`. A click on a watcher's banner
  marks the row read (`PUT /api/notifications` with the instance's cookies) and restores
  the instance at its path. **The restore hands back** what the watcher posted: it is
  merged into the page's record by a document-start script before the page's own scripts
  read it, so the restored page never posts it again.
- *The switcher, the titlebar mode, the launcher's state:* all read again when a page loads;
  a hidden page did nothing else for the shell.

**What it frees, and what it does not.** Measured 2026-10-29 on the headless rig (WebKitGTK
2.52, llvmpipe): 25 opened mock instances with the launcher showing are 26 web processes,
4.35 GB PSS (9.35 GB RSS) in all. All 25 suspended: one web process (the launcher's), 0.69 to
0.72 GB PSS (2.0 GB RSS). The network processes stay: Tauri keeps each data directory's
WebKit context alive for the app's life on Linux (tauri#14626), so each opened instance keeps
its network process (about 13 MB PSS) until the app quits. The 25 watchers cost less than the
rig resolves: stopping all 25 moved the shell process by 0.3 MB.

**How fast.** A warm restore (row click to first paint) is ~0.76 s; a cold open is ~0.80 s;
a switch to a live webview is 5 ms. A reload of the same page inside an existing webview
paints in ~50 ms, so ~0.7 s of a restore is WebKit starting the web process and creating the
webview. A pre-warm on hover could overlap at most the hover-to-click time with that, at the
cost of a web process per hovered row; it is not done.

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

Quit the installed Talaria before a dev run (Ctrl+Q, or its tray or switcher Quit — closing
the window keeps it running). The single-instance name is shared, as is the data directory, so
with it running a dev build hands over to it and exits at once: the terminal says "Talaria is
already running; brought it forward (this is a dev build: …)".

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
`libgtk-3-0`, and with the tray `libayatana-appindicator3-1`); a list in
`tauri.linux.conf.json` would only duplicate them, so the workflow asserts the built deb
declares them instead. rpm has no defaults of its own for the webview, which is why its
dependencies are written out and the deb's are not (the bundler adds the appindicator
library to both). The tray also means the Linux build job needs
`libayatana-appindicator3-dev`: the bundler finds the library with pkg-config, and aborts
the build when it cannot.

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

The window title is "Talaria — <instance>" while an instance is shown and "Talaria" on the
launcher. On Wayland the OS titlebar is a `GtkHeaderBar` that tao builds with the first title
and wraps in an event box, which keeps GTK from passing later titles to it, so the shell
sets the header bar's title itself (`commands.rs::set_header_title`). On X11 the window
manager draws the window's own title.

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

The shell posts OS notifications itself; the webview's Notification API has no permission
handler inside the shell (it reads `default` and resolves `denied`) and no site-settings
page. ui's toasts call `desktop_notify` with the row's title, body, tag (the row id), path,
kind and re-alert flag (interface I-2); a suspended instance's watcher posts the same way
(above). The shared half is `src-tauri/src/notify/mod.rs`: the clamps, the kind → urgency
and category rule (Needs-you kinds — `approval_pending`, `judge_escalation`,
`agent-problem`, `workchain_turn` — are critical; status kinds are low; DMs, mentions and
agent replies are `im.received`), the per-(instance, tag) banner map, the badge shares,
the rate limits, the status record and the click route. One backend per OS:

- **Linux** (`notify/linux.rs`) speaks `org.freedesktop.Notifications` over ONE zbus
  session connection for the whole process, with one subscription each to
  `ActionInvoked`, `NotificationClosed` and `ActivationToken`, dispatched by id. No thread
  or connection is held per banner (notify-rust held one of each until the server retired
  it: 30 open banners cost 30 bus names and 63 threads; now 0 and 0). A post carries
  app name `Talaria`, the icon and `desktop-entry` hint of the desktop file this install
  actually has — detected, never renamed: `$FLATPAK_ID` or `$APPIMAGE` →
  `app.talaria.desktop`, else the first `applications/app.talaria.desktop.desktop` then
  `applications/Talaria.desktop` across `$XDG_DATA_HOME` and `$XDG_DATA_DIRS` (deb/rpm
  ship `Talaria.desktop`, flatpak/AppImage/pacman `app.talaria.desktop`, and existing pins
  keep working) — the instance's label in front of the body, escaped when the server
  advertises `body-markup`, the urgency and category, and the server's own id from the last
  post of the same (instance, tag) as `replaces_id` (never an invented one). A re-alert is
  not a replace: the spec has a replaced notification update "with no visual cues", so the
  shell closes the old banner (`CloseNotification`) and posts a new one (GNOME Shell 50 would
  re-show a replaced one too — `notificationDaemon.js` resets `acknowledged` — but nothing
  obliges another server to). Signals count only from the server: the match rule names its
  sender, and each signal's unique name is checked against the name's owner, because a
  signal addressed to the shell skips match rules. A click comes
  with the server's xdg-activation token, which GTK hands to the compositor, so the window
  takes focus from another app on Wayland (D7) instead of only changing page. The same
  connection emits `com.canonical.Unity.LauncherEntry` for the badge (Plasma, Ubuntu Dock,
  Dash-to-Dock; stock GNOME shows none) with the detected id — Tauri's own
  `set_badge_count` assumes `Talaria.desktop` — and owns `app.talaria.desktop.Shell`: a
  second launch calls its `Activate(token)` with `$XDG_ACTIVATION_TOKEN` and exits 0 before
  it builds a window. The name is released at `RunEvent::Exit`, before the updater's
  relaunch spawns the next process, so that one never hands over to the one leaving.
- **macOS** (`notify/macos.rs`) posts through UNUserNotificationCenter in a bundled run,
  with a delegate of our own: `willPresent` answers banner + list + sound (with the window
  closed to the menu bar Talaria is often the active app, and macOS hides banners for it
  otherwise), and every click comes back by request identifier `<label>/<tag>`, whichever
  banner it is. The subtitle names the instance and the thread identifier groups its
  banners; a re-post of the same pair replaces and re-shows. Permission is asked only from a
  click — the launcher's "Allow notifications" card, or Settings → Desktop — never at
  launch; denied opens System Settings → Notifications. A post while not allowed is an
  error that names the state and the fix, and an `add` macOS refuses is an error too:
  nothing reads as success that is not one. The bundle is ad-hoc signed
  (`tauri.macos.conf.json` `signingIdentity: "-"`) as `app.talaria.desktop`, which is
  what macOS ties the permission to; desktop-package.yml asserts the signature, the
  identifier and the UserNotifications link, and runs the self-test. UN cannot run outside
  a real `.app`, so an unbundled dev run falls back to the legacy center, attributed to
  Terminal, and says so (`ns-legacy (unbundled dev run)`). mac-usernotifications
  (notify-rust's `preview-macos-un`) was read for this and not used: it routes a click only
  to a per-banner waiter registered at send time.
- **Windows** (`notify/windows.rs`) is attribution only, for now: toasts carry AUMID
  `app.talaria.desktop` — the Start-menu shortcut's when it carries that id (the NSIS and
  MSI shortcuts do), else a `HKCU\Software\Classes\AppUserModelId` entry the shell writes,
  else PowerShell's as before — and the process takes the same id.

**The click** reaches the page as a cancelable `talaria:notification-open` event with
`{href, id}` (interface I-1), dispatched by `eval` into the webview that posted the banner
(the IPC caller's label, never a value from the page). A page that handles it routes in
place, with no reload; a page with no listener — a Talaria older than it — gets
`location.assign`, as before. Only a same-origin path is ever opened, by the instance's
own rule (`openable_href` in talaria-notify): a leading `/`, no `//`, no backslash or
control character, and no `.` or `..` segment in any spelling (`%2e` included), so a link
the instance refuses is one the shell refuses too.

**What the person sees and controls.** Settings → Desktop → Notifications reads the
status from the OS (`desktop_notification_status`): "On — shown by GNOME Shell 50 (spec
1.2)", "Not allowed yet", "Blocked in System Settings", or "Notification service
unavailable: …" with the D-Bus error; that instance's own last error; Allow (macOS) or
Open System Settings; **Send test notification**, which posts through the real path and
reports what happened; and **Keep running in the background**. The launcher shows the same ask on macOS, a notice
when Linux has no notification service, the no-tray notice, and Quit.

**Closing the window** (`src-tauri/src/background.rs`, `tray.rs`) keeps Talaria running so
notifications arrive, with `closeToBackground` on (the default):

- macOS: the window hides; the dock and a menu-bar item (Open Talaria, Quit) stay; the
  dock icon reopens it.
- Linux with a tray: the window hides; the tray icon has Open Talaria and Quit Talaria, and
  the unread sum as the label beside it (appindicator has no tooltip — tray-icon's
  `set_tooltip` does nothing on Linux — and the label shows where the host draws Ayatana
  labels, as the GNOME AppIndicator extension does). A tray needs the appindicator library —
  probed with `dlopen` BEFORE any tray call, because libappindicator-sys panics without it —
  AND an `org.kde.StatusNotifierWatcher` on the bus, which the shell follows live (turning on
  GNOME's AppIndicator extension takes effect at once). If the host goes while the window is
  hidden to it (the extension turned off, updated or crashed), the window comes back, with a
  notice: its icon is gone, and a hidden window with no icon is a process nobody can reach.
- Linux without one (stock GNOME): the window **minimizes** instead — it stays in the dash
  and Alt+Tab — and a one-time notice says how to quit. A hidden window with no icon anywhere
  would be a process nobody can see or quit.
- On Linux the dash's (or a dock's) **Quit is not a way out**: the shell exports no
  `app.quit` action, so GNOME's `shell_app_request_quit` falls back to deleting each window —
  the same close request as ×, which minimizes or hides to the tray like × does. The notice
  says so. Exporting a GApplication `quit` action is the follow-up; it needs the GTK app id
  on, which changes the Wayland `app_id` the desktop files are matched by.
- Windows: Close quits, as before (no single-instance guard yet, so a hidden window would
  let a Start-menu relaunch open a second process on the same WebView2 profile).

Quit is always one step away: Ctrl+Q (Cmd+Q on macOS) in every page — injected by the
shell, so it works on any instance version, and the page on screen gets the keyboard focus
whenever the window comes back — the switcher's Quit Talaria, the launcher's Quit, the
tray's Quit. With the setting off, Close quits everywhere.

**The badge** is the sum of every instance's unread count: each page reports its own
(`desktop_set_badge`, from the bell's query, debounced to the count it settles on; a
signed-out page reports 0), a suspended instance's watcher
reports its share from each read, and removing an instance drops its share. macOS: the dock
badge, cleared at 0. Linux: the LauncherEntry above. Windows: none yet.

**Packages.** With the tray, Tauri's bundler makes the deb depend on
`libayatana-appindicator3-1`, the rpm require its library, and the AppImage carry it; the
pacman package lists it as optional. The flatpak may talk to
`org.kde.StatusNotifierWatcher` and `com.canonical.Unity`, but the GNOME runtime carries
no appindicator library, so in the flatpak Close minimizes.

**Verified how.** Linux on the headless rig (private mutter, private session bus, a
recording notification server, real input through mutter's RemoteDesktop, real
xdg-activation tokens): every check in the wave-1 acceptance list, recorded with the PR.
macOS through CI: `desktop-macos` in ci.yml compiles and lints the UN backend on every
desktop pull request, and desktop-package.yml checks the signed bundle and runs
`--notify-selftest` through LaunchServices (`backend un`, the bundle id, the
authorization, and a post that agrees with it). Windows through desktop-package.yml's
installed-exe self-test (attribution and AUMID). `--notify-selftest[=<path>]` posts one
real banner where the OS allows it: it is for CI and test sessions, not a person's own
desktop.

## Deferred on purpose

Rename/reorder instances, health badges, deep links, OAuth via external browser, Developer
ID signing and notarization (the macOS app is ad-hoc signed; Authenticode on Windows), and
launch at login. For notifications: instances not opened this session still post nothing
(D8 — a stored cookie copy would be plaintext or an ad-hoc-signed keychain item that
re-prompts on every update; the next round loads signed-in instances hidden at launch
instead); Windows' tray, close-to-background, single instance, body click and late clicks;
the Flatpak portal Notification API; and the watcher's `?since=` cursor.

macOS and Windows build and ship, but the session-isolation invariant above does not hold
there yet: `data_directory` is a no-op on those platforms (macOS wants
`data_store_identifier`), so two instances served from one host:port would share a cookie
jar. Distinct origins — every hosted instance — are unaffected.

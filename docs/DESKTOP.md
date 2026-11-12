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
Instance webviews have no Tauri drag-and-drop handler (`disable_drag_drop_handler`): the
shell listens to no file-drop events, Tauri's handler takes every drop for them (a file
dropped on a composer never reached the page), and on Windows it stops HTML5 drag and drop
altogether. The page gets drags and drops as a browser tab does — boards, the Gantt, a file
dropped on a composer. Without the handler, though, each engine's own default for a drop the
page does not take is a browser tab's: it loads what was dropped, and a file released on a
message list replaced the instance with the file — no titlebar in the themed mode, and the
escape keys are refused from a `file://` page. Two things stop that. An initialization
script (`DROP_GUARD`) answers any drag that no drop zone took with "no drop here", after
every zone in the page has seen it — except over a native field (`<input>`, `<textarea>`,
`<select>`, anything contenteditable), which takes text, a moved selection or a file by the
engine's own default, with no handler to say so; `scripts/desktop-drop-guard.mjs` runs the
script against each case in `bun run check`. And the webview refuses to navigate to `file:` at all
(`allowed_navigation`), for a page where the script did not get the chance. Pages and their
frames use `about:`, `blob:` and `data:`, and a sign-in goes to other https origins and back:
those load. `mailto:`, `tel:` and app links are not page loads, so they go to the OS
("Links that leave the page" below).

### Links that leave the page

An instance webview is a browser tab with no browser around it. A `target="_blank"` link or
a `window.open` asks the engine for a new window, and wry drops that request when the shell
installs no handler, so on every OS the click did nothing. Each instance webview now has a
new-window handler and a click script for OS links. Both are in
`desktop/src-tauri/src/links.rs`:

| The link | Where it goes |
|---|---|
| http(s), another origin or the instance's own: a Drive file, a GitHub settings page, a source the agent read, a coding-harness sign-in, a public artifact | the **default browser**, where the person is signed in to that site and has an address bar |
| the MCP connect (`/api/mcp/oauth/start` on the instance's own origin) | an **in-app popup**, the engine's own window, with the instance's session and `window.opener` |
| `mailto:`, `tel:` or `sms:`, **clicked by the person** in the instance's own page | the **OS handler** for the scheme |
| every other scheme; an OS link that arrives as a navigation or a new-window request; `file:`; `javascript:`, `data:`, `blob:` and `about:` as a new window | **nowhere**: refused and logged by scheme |

Why each one goes where it does:

- **An OS link opens only from a click.** The engine's handlers cannot tell a click from a
  script, or the page from a frame inside it:
  - WebKitGTK and WKWebView ask about subframe navigations and redirects with no frame or
    gesture flag;
  - WebView2's new-window request reaches wry's handler without `IsUserInitiated`;
  - a microsite artifact is agent-written HTML, run in a sandboxed iframe that may run
    scripts and open popups.

  So both handlers refuse every OS scheme. The click script `OS_LINKS` runs in the main
  frame only, as initialization scripts do, so never in an artifact's frame. It catches a
  trusted, primary-button click on a `mailto:`, `tel:` or `sms:` link that the page did not
  handle, and asks `desktop_open_link`. That command opens only those three schemes, and
  only the instance's own origin holds its grant. `scripts/desktop-os-links.mjs` clicks
  each case in `bun run check`.
- **The OS schemes are an allowlist of three.** Browsers hand nearly any scheme to the OS,
  but only after a prompt of their own, and the shell has none. App links have https forms
  (a Zoom or Teams join link, a Slack message link). Those go to the default browser, which
  asks before it hands them to the app.
- **One in-app window: the MCP connect.** It (`connectPopup`) needs the instance's session
  cookie to start. When it finishes, it posts `talaria:mcp-oauth-done` to `window.opener` and
  closes. The default browser has neither the session nor the opener.
  - Its path runs from the instance's `/api/mcp/oauth/start` to the provider the admin
    registered, then back to the instance's callback.
  - The popup is the engine's own window: no address bar and none of the shell's handlers.
    Every other same-origin `_blank`, such as a public artifact or a download, goes to the
    browser instead.
  - **Residual:** a page that opens the connect URL can re-navigate the window it opened.
    The shell cannot see a popup's later navigations, and on Linux the window keeps the
    start URL as its title.
- **A popup that closes itself.** On Linux, wry answers `window.close()` by destroying only
  the page, which left an empty window. The shell closes that window too
  (`links::close_emptied_popups`). On macOS, wry's popup has no handler for
  `window.close()`, so its window stays until the person closes it. That is not verified on
  a Mac.
- **Google's connects are not popups.** They are full-page navigations
  (`/api/integrations/google/connect`), so they keep loading in the instance webview, as
  before.
- **At most one hand-off a second per webview** (`HAND_OFF_GAP`). A script that opens a stream
  of windows gets one browser tab, and the rest are logged and dropped.

The hand-off uses `tauri-plugin-opener`'s free `open_url` (on Linux, `xdg-open` and its
fallbacks), on a blocking worker, never on the handler's thread. The plugin is **not
registered**: no instance page gets an opener command, an opener permission, or the plugin's
init script that intercepts link clicks. Log lines name only the scheme and host of a URL.

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
then a sentence with Retry on the launcher, not a blank window or the engine's error page —
in the themed titlebar mode (the default on every OS) the only chrome is drawn by the page,
so that page was a trap. The card also offers **Open anyway**, because the check is not the
webview: it is the shell's own HTTP client, with no cookies and its own proxy lookup and TLS
trust (on Linux, the proxy comes only from the environment). An instance behind a sign-in
gate that refuses the beacon without a session, or one reachable only through a proxy set in
the desktop's settings, fails the check and still loads in the webview.

The probe passing is not the page loading. Cloudflare holds a request to a restarting origin
for about 100 s and then answers 524, or answers 502 at once, or a WAF answers 403; a
suspended instance coming back is not probed at all; a page already open reloads into a
restart. So the shell judges every instance page itself (`src-tauri/src/loading.rs`), by
three rules, the same on every OS:

1. **A page with the Talaria UI up is never interrupted.** Up is the UI's shell hook
   (`window.__TALARIA_DESKTOP__`), or the page's titlebar asking the shell for something
   (`get_desktop_settings`, `list_instances`), which every UI's titlebar does once it has
   mounted, on the sign-in page too. The second is what keeps an instance on an older
   Talaria, from before the hook, from being judged broken. It comes late: the titlebar is
   in a chunk of its own, loaded after the main bundle and after its own import of the
   Tauri API, so a slow instance can finish its page several seconds before it asks. A
   page that is Talaria's shell still starting (its `#root` and module script, no UI yet) is
   looked at again about 8 s and 20 s after it finished before anything is said. A failed download, an
   aborted navigation or a 5xx that does not replace the page changes nothing.
2. **A page on the instance's own origin that is not Talaria is the card.** A main-frame
   page there that finished, and still has no UI 3 s later (and after those looks, if it
   was starting), puts the person back on the launcher with the card, saying what answered
   when the platform said. The card takes nothing away:
   - A page that answered — Cloudflare's 502, 524 or 403 challenge, a sign-in gate, a
     mis-routed 404, a page with no Talaria in it — is **kept** behind the card, which
     offers **Retry** and **Back to instances** (both close the page first) and **Show the
     page**: the page as it is, with the shell's own **← Instances** on it, bottom left
     (text, styled through the CSSOM in a shadow root, so a strict CSP cannot strip it), as
     the way back, which brings the card back. A page shown that way is judged no more,
     unless a later document is one the way back cannot reach (below). Show the page is the
     only way a kept page is shown as it is: opening the instance any other way — its row,
     the tray, a notification, another instance's switcher — closes the kept page and loads
     the instance fresh (Retry without the probe).
   - A load that got no answer at all (refused, dropped, an engine's network error page),
     and a page that never answers the shell's script (WebView2's own page for an error
     with no body, where no script runs; a page too busy to answer, looked at again 5 s and
     12 s later first), is closed: Retry, Back to instances. Neither offers Show the page:
     the way back could not land there.
   - Whichever it was, if the instance's UI comes up while the card is up (a challenge
     that solves itself, a slow start), the shell opens it; if it comes up on a page shown
     as it is, the way back comes down.
   - A kept page is not the instance's page: the instance's remembered page is forgotten,
     so Retry and the next launch open its root. A network failure, a stall and a page
     loaded hidden at launch say nothing against the page, and keep it.

   A page that never finishes is the watchdog's: 25 s after a webview is made, or after a
   document commits, a page still loading without its UI is "<instance> is not
   answering", with **Retry**, **Keep waiting** (show the page again, with the way back on
   it at once, and wait 60 s more) and **Back to instances**. The page keeps loading behind that
   card and is judged when it finishes: Talaria opens it; anything else (Cloudflare's 524
   after holding the request) is the card for what it is. A file the person opened in the
   window (an image, a PDF, JSON: not HTML, and nothing failed) is left alone, and Keep
   waiting on one that has finished waits for nothing.
3. **Pages on other origins are never judged.** A sign-in provider, a Cloudflare Access
   login: not Talaria, and not the shell's to call broken. A document that commits there
   starts no watchdog and is judged when it finishes only if the platform said its load
   failed on the instance's own origin. The shell takes one call from such a page, and
   only one: the way back (`show_welcome`, granted to the instance's webview for any http(s)
   origin by `lib.rs::grant_way_back`). So the pill and `Ctrl/Cmd+Shift+H` work on a sign-in
   provider's page too; `Ctrl+Q` works only on the instance's own origin (the OS's own quit,
   Alt+F4 or the macOS menu, works everywhere). Going back from another origin marks the
   page as not the instance's, so the next open loads the instance fresh instead of showing
   that page again.

**Who may take the way back, and Quit.** A page's script can do anything a page can, so
the shell decides by who is asking, never by a navigation (an earlier build of this change
navigated to `talaria-desktop.invalid` for it, which any page or frame could do; that host
is now refused like `file:`, and nothing acts on it):
- On Linux and macOS Tauri gives the IPC to the main frame only, so no frame (a
  microsite's sandboxed `srcdoc` frame, an ad) can call anything. WebView2 (Windows) gives
  it to every frame, so there a frame is held by the origin ACL and the gate below, not by
  frame scope: a sandboxed frame has an opaque origin and is refused before any command
  runs, a cross-origin frame reaches only the way back, and a same-origin frame is the
  instance's own content.
- The pill and the escape keys take only real input (`isTrusted`): a page's own
  `dispatchEvent` or `.click()` does nothing.
- `show_welcome` from an instance page is taken only for the page on screen, at most once a
  second (`loading::may_go_back`): a page in the background, or one calling it in a loop,
  cannot throw the person out of what they are using.
- `desktop_quit` from an instance page is taken only on the instance's own origin, for the
  page on screen, and not in the first 10 s after launch (`loading::may_quit`): no page can
  quit the app from the background, or on every start.

Off screen, a page that does not come up is closed. One loaded hidden at launch (D8) is
loaded again 1, 2, 5, 15 and 30 minutes later, so its instance's notifications are not lost
for the session; a page left behind on the card is closed when the person opens another
instance.

What the platform says, kept for the card's words only (the rules above decide):

- **Linux:** WebKitGTK's `load-failed` for an http(s) main-frame load that failed
  (`commands.rs::watch_load_failures`; the `webkit2gtk` crate is a Linux-only dependency),
  and the main resource's status when a document commits. The failed load is answered
  "handled", so WebKit shows no error page of its own and a page that was up stays up; a
  navigation that was replaced, a download, a link WebKit hands elsewhere (`mailto:`) or
  content it cannot show inline says nothing (left to WebKit, a `mailto:` link replaced the
  whole instance with "The URL can't be shown"; the navigation handler now refuses it, and
  one the person clicks goes to the OS, links.rs).
- **Windows:** WebView2's main-frame `NavigationCompleted`, subscribed through
  `with_webview` and webview2-com (`watch_navigation`, `loading::webview2`), for an http(s)
  page: its `HttpStatusCode` from 400, or `IsSuccess` false with no HTTP answer at all —
  the network error page, where no init script runs and Alt+F4 was the only way out. A
  cancelled navigation (`OPERATION_CANCELED`: replaced, or stopped) and one that became a
  download (`CONNECTION_ABORTED`) say nothing.
- **macOS:** nothing: WKWebView tells only its navigation delegate, which is wry's, what a
  page answered, and WebKit's Navigation Timing has no `responseStatus`. Rule 2 needs none —
  the card names no status there. A load that fails before it commits never finishes on
  macOS; the watchdog takes it.

Each engine callback only records; every decision is a main-thread turn of its own.

### Reloading a page

An instance page has no browser chrome, so the shell provides reload itself. It works on any
instance, whatever its Talaria version.

| Where | How |
|---|---|
| The keys | Ctrl+R or F5 on Windows and Linux, Cmd+R on macOS. Ctrl+Shift+R, Shift+F5 and Ctrl+F5 (Cmd+Shift+R on macOS) also skip the HTTP cache. |
| The in-UI switcher | **Reload page**, with the shortcut beside it (`DesktopSwitcher.svelte` → `shellReload`). |
| macOS menu bar | **View → Reload Page** and **Reload Ignoring Cache**. |

How each path works:

- **The keys** are an initialization script the shell injects into every instance webview,
  beside the escape hatch (`RELOAD_KEYS` in `desktop/src-tauri/src/reload.rs`).
  - The script calls `desktop_reload`, so it also runs on a page whose Talaria UI never
    loaded, such as a gateway's 502 page.
  - Where the shell refuses the command (another origin, which has no grant), the script
    reloads the page itself.
  - `scripts/desktop-reload-keys.mjs` presses each key, on each OS, in `bun run check`. It
    also checks the chords the script must leave to the page.
  - The keys work in the page's main frame only, as initialization scripts run. With the
    focus inside an iframe (the artifact preview, the Google file pane), Ctrl/Cmd+R does
    nothing on Linux and macOS.
  - The script takes the keys in the capture phase, so a page's own Ctrl/Cmd+R binding never
    runs. Nothing in `ui/` binds one.
  - A popup (macOS hands the connect popup the opener's scripts and IPC) reloads itself
    rather than the instance page behind it.
- **The switcher** calls `desktop_reload`. A shell that predates the command gets an ordinary
  `location.reload()`.
- **The grant.** `desktop_reload` is in `INSTANCE_PERMISSIONS`, so every registered instance
  origin holds it. A page can reload only itself with it. The launcher's call reloads the
  instance on screen.
- **The macOS menu** is the app's own, so it works where no page script ran. A page that
  already handled Cmd+R keeps the menu from reloading a second time.
- **On Windows**, WebView2's own F5 and Ctrl+R still reload its error page, where no script
  runs.
- **Skipping the cache** uses WebKitGTK's `reload_bypass_cache` and WKWebView's
  `reloadFromOrigin`. WebView2 has no such reload outside the DevTools protocol, so on
  Windows it is an ordinary reload.
- **When a load fails on Linux**, the shell still comes back to the launcher's failure card
  and its Retry ("Opening an instance" above).

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
  `snooze_update`, `desktop_notify`, `desktop_notification_status`,
  `desktop_notification_request`, `desktop_notification_test`, `desktop_set_badge`,
  `desktop_quit`, `set_close_to_background` and `desktop_suspend_report`. A suspend report counts only as
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

## Threading

Window and webview operations run on the main thread, in the order the geometry section
above needs. One more rule decides **where** a webview may be made: **no IPC command may
create a webview, or wait on WebView2, inline.** A sync `#[tauri::command]` runs inside the
calling webview's IPC callback, and on Windows that callback is a WebView2 event handler.
A WebView2 created there waits for a controller that WebView2 cannot deliver while its own
handler is on the stack, and wry waits for it in a message pump with no timeout
([Microsoft's reentrancy rule](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/threading-model#reentrancy);
Tauri says the same on `WebviewBuilder::new`). Through desktop v0.11.0 `activate_instance`
was such a command: on Windows every first open of an instance — Add → Connect, the reopen
at launch, a cold or suspended switch — froze the app for good, while the window kept
repainting. Linux and macOS create synchronously, so nothing there could see it.

So every path that makes an instance webview (`activate`, `prewarm` in `commands.rs`) runs
as a main-thread task of its own, the whole hide → create → show sequence in one turn:

- `activate_instance` is `async` and hops with `on_main_turn`, which awaits the answer on
  its tokio worker and never blocks the main thread.
- Code that may already be on the main thread queues with `queue_main_turn`: a restore from
  a window event (on Windows a window operation inside a sync command raises those
  synchronously), a notification click, a tray row. `run_on_main_thread` is not enough
  there — Tauri runs its closure inline when it is called on the main thread.
- A second activation of an instance whose webview is still being created changes nothing
  (`Creating`), so no label is ever created twice.
- Reading a webview's cookies waits on WebView2 too: it runs on a blocking thread
  (`read_cookie`).

Two caveats. A queued turn runs from tao's own message loop, not inside a WebView2 handler,
only as long as no sync command runs a nested message pump of its own inside a WebView2
callback — none does (a window drag posts its message). And two queued turns are two tokio
tasks, so they are not strictly first-in, first-out: an activation and a Ctrl+Shift+H a few
milliseconds apart can land in either order. Nothing depends on that order.

`bun run check` holds the rule on every OS: `scripts/desktop-threading.mjs` builds a call
graph of the shell and fails when a sync command can reach a webview creation or a cookie
read, when an async command can reach one from its tokio worker, or when a callback can
reach one inline. Callbacks are the closures handed to `.on_*(…)` (window, webview,
page-load, navigation, menu and tray events), `.run(…)`, `.with_webview(…)` and GTK
`connect_*` signals, plus any function marked `// webview-threading: callback` (the
notification-click entry points the OS calls, and WebView2's `NavigationCompleted`
handler). It follows free function calls by name, not
method calls or macro expansions: a regression guard for this shape of bug, not a proof.
CI's connect smoke proves the connect path on every OS (Gates, below).

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
  gets its answer within 2 s. On Windows the same thread is WebView2's: no IPC command may
  create a webview or wait on WebView2 inline ("Threading" above).

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
  store, kept in memory, sent only to its origin, redirects not followed), made live by
  `/api/me/events` with the page's 30 s poll floor. The first read is taken while the page
  is still open: if it fails for anything but a 401/403 (a proxy or certificate only the
  webview knows, the instance down), the page is kept. It runs only when the page said it
  would post (ui's `browserNotifyEnabled`), and keeps the page's own arrival rule
  (docs/notifications/contract.md, I-20): a row with a `seq` is posted when it is unread,
  past the seq the watcher has read through, and unseen — or seen at an earlier
  `createdAt`, which is a re-alert under the same tag; a row with no seq (filed before the
  contract) keeps wave 1's rule, newer than the high-water mark less two minutes. With
  `since` in the instance's `features`, the watcher reads `GET /api/notifications?since=<seq>`
  page by page and moves its seq to the snapshot of a page whose `more` is false — or, when
  a catch-up outruns its ten pages (2,000 rows), to the last row read, so the next read goes
  on from there instead of starting over and posting again — and its stream asks
  `/api/me/events?since=<seq>` so the server replays what a cut lost (I-19): a reconnect
  after an outage is exact, never "the newest 50". A complete page whose snapshot seq is
  BELOW the watcher's — the instance's database was restored or reset under it — makes the
  watcher rebase, as the page does: it reads the newest page, judges it by wave 1's rule
  (the high-water mark less two minutes, and the rows it has seen), and takes the server's
  seq, so what was filed since the restore is posted instead of waiting for the server to
  pass the old seq; a `resync` frame below its seq asks for that read at once.
  **It starts from the page's record**,
  `localStorage['talaria.notify.arrivals.v1']`, which the suspend report hands over
  (`arrivals`, with the record's `v` and `seq`): whatever the page missed — a dead stream,
  a sleep — is posted on the watcher's first read instead of being swallowed into a new
  baseline (D2). A page with no record (an older UI) gets the old baseline. More than three
  at once are one "N new in <instance>" banner tagged `talaria-summary`, the page's own
  summary tag (I-21): its click restores the instance and hands the restored page
  `talaria:notification-open {id: 'talaria-summary'}` once its listener is up, which opens
  the bell. **What it posts** (I-15): never a row whose `alert` is `feed`; while the
  person's Do Not Disturb is active (`dnd.active`), only a row of a class that breaks
  through it (`classes[].breaksThrough`, and `dnd.allowNeedsYou`); nothing for an instance
  muted in the launcher; and nothing while the person's presence winner (I-17) is another
  of their clients — a browser tab they are looking at, another desktop — which alerts
  instead, as the page runtime's cross-device rule has it. The winner comes from `presence`
  frames, each heartbeat's answer, and one `GET /api/me/presence` before the first post; the
  watcher's own id and its instance's page here (the page's `talaria.notify.client`, from the
  suspended session) count as this desktop. Every row it posts is acked (`POST
  /api/notifications/ack`, I-18), so the instance's deferred push for it is cancelled; an
  ack that never reached the instance (a transport error, or a gateway's 502/503/504, as a
  Cloudflare tunnel answers while the origin restarts) is sent once more 2 s later — a lost
  one lets that push fire 60 s on, and the ack is idempotent. **Read-sync** (I-16): a
  `notifications` frame that names `readIds` (or `all`) closes this instance's banners for
  those rows — keyed by its label, so another instance's read never closes them — and
  `resync` closes every banner whose row a fresh read finds read. **Presence** (I-17):
  every 30 s the watcher heartbeats `PUT /api/me/presence` as `desk-<instance id>`, kind
  `desktop`, `active` while the OS saw input in the last two minutes (`desktop_os_idle`,
  below), else `idle`, so the instance's push waits while the person is at this desktop
  and does not while they are away; an instance that predates the presence store gets no
  heartbeat (it would read any PUT as "online", all night). A failed stream — a 502 while
  the instance restarts, a deploy, Cloudflare dropping an idle connection — is reopened
  with backoff (1 s doubling to 30 s, reset after a minute up), every reopen reads the rows
  again, and the 30 s poll floor and the heartbeat keep running through the outage; a
  401/403 from the instance itself (its JSON) ends the watcher, while a Cloudflare
  challenge (`cf-mitigated`, an HTML 403) counts as an outage — the shell's requests also
  name themselves (`User-Agent: Talaria-Desktop`), which a bot filter wants. Each read,
  and each `notifications` frame, sets the instance's share of the badge: For you
  (`unreadForYou`), or `unread` from an instance that predates it. A click on a watcher's
  banner marks the row read (`PUT /api/notifications` with the instance's cookies) and
  restores the instance at its path. **The restore hands back** what the watcher judged
  and the seq it read through: both are merged into the page's record (`v: 2`) by a
  document-start script before the page's own scripts read it, so the restored page never
  surfaces them again and reads on from where the watcher stopped. An instance that serves
  no `features` gets wave 1's watcher exactly.
- *The switcher, the titlebar mode, the launcher's state:* all read again when a page loads;
  a hidden page did nothing else for the shell.

**Instances nobody opened (D8).** At launch, every registered instance with a cookie jar
(it has been opened, so it may be signed in) that the window will not show is loaded
hidden — the first a second after launch, the rest five seconds apart, five at most
(O-17) — and asked as soon as its page has loaded, not after the timeout: its page
answers, the shell reads its cookies into memory (nothing is stored at rest) and starts its
watcher, and the webview closes like any suspended one. So a signed-in instance alerts
whether or not the person opens it this session, as Slack and Teams do for every signed-in
workspace. A page still booting when asked is asked again on the next tick (six times at
most); one that fails to load is dropped quietly, with no card on the launcher; with
suspending Off, the page stays loaded, hidden, and posts its own. The cost is a page load
per instance at launch, then a watcher each.

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

A devbox has no display, so the split is: **code in the box, gates on CI, GUI on the host**
(the box's clone is host-visible at `~/Development/devboxes/<name>/talaria`).

```sh
# host, once: the webkit runtime (Arch; gtk3 rides along)
sudo pacman -S webkit2gtk-4.1 librsvg

# host: image (carries the tauri build deps) and the box
bun talaria box build
bun talaria box new desktop --env GH_TOKEN=…   # gate:remote's token: a box has no gh login

# in the box — the gates run on GitHub, not here: commit, then, in the background
bun talaria box enter desktop
bun run gate:remote          # light: the Linux desktop job
bun run gate:remote --full   # adds desktop-macos and desktop-windows

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
`cargo test` + `svelte-check`, the same list CI's `desktop` job runs. Like `api:check`, it
is CI's job, never local, and `bun run verify` does not include it. The proof is the pull
request's CI, or `bun run gate:remote` before one exists. A light run (`gate:remote`) has
only the Linux `desktop` job; `bun run gate:remote --full` adds `desktop-macos` and
`desktop-windows`, which a change to macOS or Windows `cfg` code needs before a PR.

CI runs clippy and the tests on all three platforms for every desktop pull request:
`desktop` (Linux), `desktop-macos` and `desktop-windows`, because each OS's `cfg` code is
compiled by no other job. Clippy and tests cannot see a hang, so each of the three jobs
then builds the shell with the launcher embedded and runs the **connect smoke**
(`desktop/packaging/connect-smoke.mjs`; Linux under `xvfb-run` and `dbus-run-session`): a
fake instance on `127.0.0.1` serves the beacon and a page whose script fetches `/opened`,
the registry names it as the last view, and the smoke launches the shell and passes only if
the page is requested and its script runs within 90 s with the shell still up — boot,
probe, activate, webview, navigation. A pass exits as soon as the page answers; the wait is
long because release.yml runs these jobs before every publish. On a
failure it says which step happened last, and prints the instance's data dir and
`lastOpenedAt` (a frozen open leaves the dir made and the timestamp null) and the shell's
log. It writes the shell's real data dir, so it refuses to replace an existing registry
without `--force`: run it on a throwaway runner or under a private `HOME`.
`desktop-package.yml` runs the same smoke on what it ships, with `--force` on its
throwaway runners: the exe the Windows installer installed, the binary inside the macOS
`.app`, and the binary the Linux deb installed (under `xvfb-run`), so a release build that
freezes on its first open fails the job before anything is attached to a release.

## Logs

Everything the shell says once it has started (`eprintln!` in `desktop/src-tauri`), and any
panic, also goes to a log file, so a release build with no console — Windows, a Finder or
Start-menu launch, a `.desktop` entry — still leaves evidence. Lines from before setup (a
second launch on Linux handing over to the first, `--notify-selftest`) go to stderr only.
The file:

| OS | File |
|---|---|
| Windows | `%LOCALAPPDATA%\app.talaria.desktop\logs\talaria-desktop.log` |
| macOS | `~/Library/Logs/app.talaria.desktop/talaria-desktop.log` |
| Linux | `~/.local/share/app.talaria.desktop/logs/talaria-desktop.log` (flatpak: under `~/.var/app/app.talaria.desktop/data/`) |

Each line carries a UTC timestamp; each launch starts with the version and the OS, and a
panic's message and location are written through a panic hook before the default one runs. At
about 1 MB the file becomes `talaria-desktop.log.1`, replacing the one before, so the two
files stay near 2 MB. Ask a person reporting a problem for both; no line carries a cookie
or a session value. The sink is
`desktop/src-tauri/src/logfile.rs`; the crate's own `eprintln!` macro (lib.rs) mirrors
every call site to it.

## Installers

`.github/workflows/desktop-package.yml` builds every installer and attaches it to a
`desktop-vX.Y.Z` release: a push to main that touches `desktop/` mints the next one, and a
`publish=true` dispatch mints a hand-picked number (RELEASING.md has the operator's half).
Those are the only releases that carry installers. A server tag (`vX.Y.Z`, `vX.Y.Z-rc.N`)
builds none: Desktop has its own version line, and an installer stamped with the server's
0.13.0 beside `desktop-v0.12.0` would never be offered an update until Desktop passed 0.13.

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
that claims to be `0.2.0`. Every file name takes the tag's version verbatim
(`Talaria_0.2.0-rc.1_amd64.deb`, `Talaria-0.2.0-rc.1-1.x86_64.rpm`, the dmg, the flatpak
bundle name), but inside, the Linux packages spell a pre-release so it sorts below its
release: written as the tag has it, dpkg reads `0.2.0-rc.1` as 0.2.0 with Debian revision
`rc.1`, and rpm and pacman read the extra `rc` as newer, so all three ranked an RC above
its release and refused the release over it as a downgrade. The deb and rpm `Version` is
`0.2.0~rc.1` (`~` sorts before everything, the end included; `recommend-tray.sh`, which
checks the order with dpkg's and rpm's own compare), and the pacman `pkgver` is
`0.2.0rc.1` (pacman has no `~`, but a trailing segment that starts with a letter loses to
the end; `pack-pacman.sh`). rc.1 < rc.2 < rc.10 holds in all three, and so does
`0.0.0~sha…` < `0.0.0` for a trunk build.

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
different claims, so the pipeline makes the runners prove what they can: macOS verifies the
dmg and launches the app, windows installs whichever installer the run produced and
launches what it installed, and linux installs its deb. Each then runs the connect smoke
(see Gates) on that shipped binary, which passes only if the app opens an instance;
"still running twenty seconds later", the check the pipeline began with, is also what a
frozen app passes. When this pipeline was written, the Linux formats were installed in clean
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
the OS — left on macOS, right elsewhere) and a bar you can drag. Inside an instance that bar
is the tab bar: it holds the controls, a press on its empty space drags the window and a
double-click maximizes it, while a press on a link, button or field does what it does in a
browser. Screens with no tab bar (sign-in, the wait for the session) draw a 36px band of
their own instead. OS turns native decorations
on. None hides both. Settings → Profile (inside the desktop app) and the launcher welcome
screen both set it. Turning the OS titlebar on keeps the window's outer size (the content
loses the titlebar's height), and the shell re-places the visible webview straight away
rather than a frame later, which used to flash a band of bare window.

The window opens at 1280×800, shrunk to fit the monitor it opens on when that is smaller
(`layout::fit`: a 1440×900 panel at 167 % is 864×540 logical, and the fixed size put the
themed min/max/close off-screen). The minimum is 760×480, also shrunk to fit.

## Updates

A stable GitHub Release carries `latest.json` plus minisign signatures of the AppImage,
the universal `.app.tar.gz`, and the NSIS installer. The running app fetches
`/releases/latest/download/latest.json` (on its own, below, or when someone checks in
Settings → Profile or on the launcher), verifies the payload against the pubkey in
`tauri.conf.json`, replaces itself, and relaunches. The check is bounded at 10 s (the
plugin's default is no bound); the install is not, because the plugin applies one timeout
to the check and to the whole download.

`/releases/latest` is whichever release GitHub has marked **Latest**, and that is always a
`desktop-v` release: `desktop-package.yml` creates its mint with `--latest=false` and marks
it Latest only after its `latest.json` is uploaded, so the feed never points at a release
without one. A platform release (`vX.Y.Z`, `release.yml`) is never Latest: it would take the
flag by default the moment it opened, and it carries no installers and no `latest.json`, so
every update check would 404. `check-invariants.mjs`
(`release-latest-is-the-desktop-feed`) holds both workflows to this.

**The update chip** (`src-tauri/src/update.rs`). Nobody had to find "Check for updates"
for the app to learn about a release: the shell checks on its own, 30 s after launch (after
the launch-time loads) and then every 6 hours by the wall clock, so a laptop that slept
through the interval checks when it wakes; an hour after a check that failed. A failed
background check is a log line, never a chip. A dev build never checks on its own (its
version is the decorative one in `tauri.conf.json`). The check runs on a tokio worker, and
everything it changes on screen is a main-thread turn of its own. When the feed has a newer
version, a chip reads "Talaria Desktop 0.13.0 is available", with **Update** and **Later**:

- **On the launcher** it is a toast in the corner (`UpdateChip.svelte`), from the snapshot
  (`ShellSnapshot.update`).
- **Over an instance** the shell evaluates the same chip into the page (`update::chip_js`):
  a shadow root on `<html>`. Instances run different Talaria versions, so the chip measures
  the page it lands in, a moment after it arrives, on resize and every 2 s — never while the
  pointer is on it or a press is in progress (it measures again once the pointer leaves),
  and not at all once it is gone: Later, a snooze or no offer stops its timer and resize
  listener with it:
  - **Themed** (the default), **where the top band's middle takes no click** — an older
    Talaria's 36 px titlebar, a sign-in page, the tab bar's empty middle on a wide window:
    one 28 px row (the version, Later, Update) centred 4 px from the top. That band has the
    window controls at one end and empty drag space in its middle, so the row covers nothing
    a person types into or reads, and the window still drags from either side of it.
  - **Otherwise** — a UI whose tab bar is the titlebar with its tabs in the middle, the OS
    titlebar, or none: the chip starts as its small pill just below the bar across the top
    of the page (62 px if there is none to measure), where nothing under it takes a click:
    the middle, else a quarter or an eighth in from either side, else across the bar's
    lower edge (on a small window zoomed in, where the sidebar's search and the header's
    buttons fill the row below the bar). If no spot is free it takes the middle, below the
    bar. Opened, its row sits centred below the bar, over the top of the page's header line,
    until it is folded again; on a 1000 px window that covers a header button (rc's star) by
    3 px, and on a small window zoomed in to 175 % the end of the sidebar's search. Changing the titlebar mode moves it at once
    (`update::repaint`).

  Its **–** makes it a small pill at any time, an install included: the pill keeps the
  download's progress, and nothing is cancelled; a progress push changes the text in place,
  so a click or the focus is never lost to a re-render. An install that fails folds it to
  "↑ Update failed" until the person opens it. A second line (the failed install's sentence,
  the **How?** sentence) drops below the row only when the person asks for it, and then it
  is over the page's nav until folded. A window too narrow for the row (a small window, or a
  page zoomed in: below 560 CSS px) starts folded, and the row never spills over the window
  controls. It is styled from a constructed stylesheet, which a page's CSP does not govern,
  and if a sheet ever does not apply, the same rules are set through CSSOM (`el.style`),
  which no CSP refuses. Text is set as text, and it never takes the focus. It is the shell's,
  not the instance UI's, because the instance webview fills the window and its UI is on its
  own release cadence: a chip that went through the desktop bridge into ui's toast stack
  would show only on instances already running this release, and nowhere while the page is
  half up. A native OS banner was the other way, and it is worse at this: it needs the OS's
  permission (macOS asks first), Windows routes only its button and loses the click after a
  restart, and a banner cannot hold a Later that every view honours. The chip is re-added
  when a page loads (`on_page_load`), and only to a page on the instance's own origin, never
  to a sign-in provider the page went to.
- **Later** puts that version off for 24 hours, in `settings.json`
  (`updateSnooze: {version, untilMs}`): the chip comes down in every view at once, and a
  relaunch keeps it put off. A newer version than the one put off is offered at once. A
  snooze that ends more than a day out (the clock went back, a hand-edited file) has run out.
- **Update** is `install_update`. The chip says the install is running, how much of the
  download has arrived, and, if it fails, why, in a sentence, in every view (a second click
  while it runs is refused). Its look at the feed is bounded at 20 s, and the download by its
  progress, on the monotonic clock: nothing arriving for 60 s stops it with a sentence and
  Update offered again. There is no short total — the Linux AppImage is about 83 MB, 43
  minutes at 256 kbit/s — only a 4-hour backstop for a download that trickles forever, whose
  sentence points to the releases page. Installing the bytes once they are here is local and
  never cut off, and however `install` ends, a panic included, the chip stops saying
  "Updating" (`InstallRun`). On success
  Talaria restarts: on Linux and macOS through the app's own exit, so every page is
  remembered and the single-instance name is free first; on Windows the NSIS installer runs
  passively (`installMode`), and relaunches the app when it is done.

**No Update where the updater cannot replace the copy.** The feed has one Linux entry, the
AppImage, and one Windows entry, the NSIS setup, and the updater falls back to them for
every bundle. A deb or rpm install would download the AppImage and refuse it ("invalid
updater binary format", after the whole download), the pacman package and the Flatpak are
read-only, and an `.msi` install would get a second, per-user NSIS copy beside it while its
Start-menu entry kept the old one. So Update is offered only to an AppImage (`$APPIMAGE`,
as the updater itself reads it), the NSIS install, and the macOS app (not one running from
its disk image or a quarantine copy, which it cannot replace). Everywhere else the chip, the
launcher's Check for updates and Settings → Profile say how to update instead — the new
package or `.msi` from the Releases page — and the chip offers Later only; `install_update`
refuses with the same sentence, for an older instance UI that still shows Install.

Signing uses `TAURI_SIGNING_PRIVATE_KEY`, a secret of the `desktop-release` GitHub
environment, which only `main` reaches. Once the repository-level copy is deleted, a build
from any other ref is unsigned (RELEASING.md, "The desktop installers"). Losing that key
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
  actually has — detected, never renamed: `$FLATPAK_ID` → `app.talaria.desktop`; an
  AppImage (`$APPIMAGE`, mounted at `$APPDIR`) → the `Talaria.desktop` at its mount's root,
  with the icon as a path into the mount (`$APPDIR/talaria-desktop.png`, the `Icon=` name
  the AppImage spec puts at the root, else `.DirIcon`), because nothing installs either and
  a notification server looks an icon name up only in its own theme;
  else the first `applications/app.talaria.desktop.desktop` then
  `applications/Talaria.desktop` across `$XDG_DATA_HOME` and `$XDG_DATA_DIRS` (deb/rpm
  ship `Talaria.desktop`, flatpak/pacman `app.talaria.desktop`, and existing pins
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
  else PowerShell's as before — and the process takes the same id. The OS idle time comes
  from `GetLastInputInfo`; a read elsewhere closes no toast yet.

**Read-sync and presence, from the shell** (I-16, I-22). Two commands every instance page
may call, keyed by the caller's label:

- `desktop_notify_close(tags)` closes that instance's banners for these tags (at most 100
  a call; ui calls it with the `readIds` of a read elsewhere). Linux: `CloseNotification`
  for the id the server issued for each (instance, tag). macOS:
  `removeDeliveredNotificationsWithIdentifiers(["<label>/<tag>"])` (and pending ones), so
  the banner leaves Notification Center's list too. Another instance's tags never reach
  them: the label is the IPC caller's, never a value from the page. The watcher closes its
  own banners the same way, from its stream.
- `desktop_os_idle()` answers the milliseconds since the person last touched the machine,
  or null where the OS cannot say; one reading serves every caller for half a second. Linux: mutter's `org.gnome.Mutter.IdleMonitor`
  `GetIdletime` on GNOME, else `org.freedesktop.ScreenSaver.GetSessionIdleTime` — KDE's
  kscreenlocker answers it on X11 in milliseconds (KIdleTime's), though the interface's XML
  calls the value seconds, and refuses it on Plasma's Wayland — else null. macOS:
  CoreGraphics' `CGEventSourceSecondsSinceLastEventType` for the whole login session.
  Windows: `GetLastInputInfo`. The flatpak may talk to both Linux services.

**The click** reaches the page as a cancelable `talaria:notification-open` event with
`{href, id}` (interface I-1), dispatched by `eval` into the webview that posted the banner
(the IPC caller's label, never a value from the page). A banner whose instance the click
restores from suspension is born at its row's path; the watcher's summary, which names no
row, is born at the instance's last page and hands it `{href: null, id:
'talaria-summary'}` through a document-start script that offers the event until the page's
listener takes it (20 s at most, once per restore) — the page opens the bell. A page that handles it routes in
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
reports what happened; **Keep running in the background**; and, for an instance muted in
the launcher, that it is muted and that its count still counts toward the badge. The
launcher shows the same ask on macOS, a notice when Linux has no notification service, the
no-tray notice, Quit, and on each instance's row its unread count and a bell that mutes
it: a muted instance posts no banner, from its page or its watcher, and its share of the
badge still counts (registry `muted`, never read by another instance). An instance asks
too, because a person who resumes straight into one never sees the launcher (every 0.11
upgrader): while the shell reports `not-determined` (macOS alone does), the page shows a
card once it has been open 10 s and touched, whose Allow runs the macOS prompt inside that
click (`ui/src/components/app/DesktopNotifyPermission.svelte`, `createShellAsk` in
`ui/src/lib/notify-prompt.ts`); Not now is remembered for that instance.

**Closing the window** (`src-tauri/src/background.rs`, `tray.rs`) keeps Talaria running so
notifications arrive, with `closeToBackground` on (the default):

- macOS: the window hides; the dock and a menu-bar item stay; the dock icon reopens it.
  The menu-bar item is a template image (the wing mark in black, which the menu bar tints)
  and a left click shows its menu, as every menu-bar item does.
- Linux with a tray: the window hides; the tray icon's menu has a row per instance with its
  own unread count ("Acme — 3"; a click opens that instance; a read anywhere drops it; a
  count changes its row's text in place, because a new menu makes libayatana register the
  item with the host again), Open Talaria and Quit Talaria, and the unread sum as the label beside it (appindicator
  has no tooltip — tray-icon's
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
- On Linux the dash's (or a dock's) **Quit quits**. The GTK application is registered as
  `app.talaria.desktop` (`enableGTKAppId` in tauri.conf.json) and exports an `app.quit`
  action (lib.rs `install_quit_action`): GNOME's `shell_app_request_quit` activates it —
  every page remembered, then exit 0 — where before it found none and fell back to deleting
  each window, the same close request as ×. The GApplication's bus name
  (`app.talaria.desktop`) and the shell's single-instance name
  (`app.talaria.desktop.Shell`) are two names on two connections, and a second launch still
  hands over on the latter before GTK starts. The registered application also answers
  D-Bus `org.freedesktop.Application.Activate` (`gapplication launch app.talaria.desktop`,
  a launcher that activates by GApplication): tao's start handler would hand tauri a second
  start, and tauri would run its setup again and panic on the existing window, so that
  handler is blocked once the app is up and an activate shows the window instead. With the
  app id on, the window's Wayland
  `app_id` is `app.talaria.desktop`, and so is its X11 `WM_CLASS` (the program name is set
  to it), so a dock matches it to `app.talaria.desktop.desktop` (flatpak, pacman)
  directly and to the deb/rpm/AppImage `Talaria.desktop` by its `StartupWMClass` (the
  bundler's desktop template is `packaging/linux/Talaria.desktop.hbs`, the default one with
  that line). No file is renamed, so a pin keeps its desktop-file id (O-16). An AppImage
  integrated under v0.11.0 (Gear Lever, AppImageLauncher: each copies the embedded file
  once) keeps that era's `StartupWMClass=talaria-desktop`, and the in-app updater replaces
  only the AppImage, so after the update its window no longer groups under that pin:
  integrate it again. GLib's application name is `Talaria` (lib.rs `name_the_process`): it
  is the tray item's StatusNotifierItem `Title`, the entry's name in Plasma's tray, which
  otherwise falls back to the program name `app.talaria.desktop`.
- Windows: Close quits, as before (no single-instance guard yet, so a hidden window would
  let a Start-menu relaunch open a second process on the same WebView2 profile).

Quit is always one step away: Ctrl+Q (Cmd+Q on macOS) in every page — injected by the
shell, so it works on any instance version, and the page on screen gets the keyboard focus
whenever the window comes back — the switcher's Quit Talaria, the launcher's Quit, the
tray's Quit. With the setting off, Close quits everywhere.

**The badge** is the sum of every instance's For you count — what needs the person and
what is addressed to them (`unreadForYou`; EB-11, O-10); Following lands in the bell's All
tab without badging the dock, and an instance that predates For you counts every unread
row: each page reports its own (`desktop_set_badge`, from the bell's query, debounced to
the count it settles on; a signed-out page reports 0), a suspended instance's watcher
reports its share from each read and each read-sync frame, and removing an instance drops
its share. macOS: the dock badge, cleared at 0. Linux: the LauncherEntry above. Windows:
none yet.

**Packages.** The tray library is a recommendation, never a requirement: the shell opens
it with `dlopen` and runs without it (Close then minimizes). With tauri's `tray-icon`
feature the bundler makes the deb depend on `libayatana-appindicator3-1` and the rpm
require `libayatana-appindicator3.so.1()(64bit)`, and no config removes them, so
tauri.linux.conf.json lists the library under `recommends` for both and desktop-package.yml
takes it back out of Depends and Requires (`packaging/linux/recommend-tray.sh`: the deb's
control file edited in place, the rpm rebuilt from its own payload and header with the same
files), then checks both. The AppImage carries the library; the pacman package lists it as
optional. The flatpak may talk to `org.kde.StatusNotifierWatcher` and `com.canonical.Unity`,
but the GNOME runtime carries no appindicator library, so in the flatpak Close minimizes.

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
launch at login. For notifications: Windows' tray, close-to-background, single instance, body click, late
clicks, closing a toast on a read elsewhere and the taskbar overlay badge (cut from wave 2's
desktop round, Linux and macOS first); the Flatpak portal Notification API and a bundled
appindicator library in the flatpak; and a stored cookie copy for instances, which stays
out on purpose (it would be plaintext, or an ad-hoc-signed keychain item that re-prompts on
every update — unopened instances are loaded hidden at launch instead).

macOS and Windows build and ship, but the session-isolation invariant above does not hold
there yet: `data_directory` is a no-op on those platforms (macOS wants
`data_store_identifier`), so two instances served from one host:port would share a cookie
jar. Distinct origins — every hosted instance — are unaffected.

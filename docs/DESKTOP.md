# Talaria Desktop

The desktop app is a **multitenant shell** around Talaria instances — the connect-out half of
the roadmap's business-multitenancy line ("swap between them in a click … connect out to other
hosted Talaria instances"). It is deliberately NOT a second UI codebase: the interior of every
instance is that instance's own web UI, loaded live from its origin. The only local frontend
is the sidebar.

Built on [Tauri v2](https://v2.tauri.app/) (Rust shell + WebKitGTK webviews on Linux), tracked
in [`desktop/`](../desktop).

## How it works

One window, webviews in two roles:

- **The launcher** (webview label `main`, local content from `desktop/src`) is the
  full-window welcome screen until an instance is active, then a 240px sidebar. Its layout
  mode is derived from its own state — the same state that drove the activation — never
  inferred from viewport width (scale-factor → CSS-pixel conversion under WebKitGTK is
  exactly the arithmetic that strands a strip showing the wrong layout). The window is
  `decorations: false` for the same respect-the-WM reason: GTK would otherwise draw a
  client-side titlebar even where the window manager (Hyprland et al.) shows none.
- **One webview per instance** (label `instance-<id>`, remote content at the instance's
  origin). Each gets its own `data_directory`, so cookie jars and localStorage never mix —
  cookies ignore ports, so without this, two dev instances on `127.0.0.1:530x` would silently
  share a session. Hidden instance webviews stay loaded: sessions and SSE survive a switch.

Adding an instance: the shell normalizes the URL (bare hosts become `https://`, only
http(s), credentials/paths stripped — an instance IS its origin), then probes the instance
beacon (`GET /api/well-known/talaria-instance` → `{instance, companyName}`). A valid beacon
uuid is what makes a URL "a Talaria instance"; the uuid is also the dedupe key. The label
comes from `companyName`, falling back to host(:port).

The registry is `~/.local/share/app.talaria.desktop/instances.json` (atomic writes), and each
instance's session data lives beside it in `instances/<id>/webview/`. Removing an instance
deletes its data dir — the saved session dies with it; the instance itself is untouched.

## Security posture

- **Instance webviews have zero IPC.** Every shell command is declared in
  `desktop/src-tauri/build.rs` (`AppManifest::commands`), which gates commands behind
  capabilities; only `capabilities/launcher.json` exists, scoped to the launcher webview.
  A page loaded from an instance origin cannot invoke Tauri commands at all — there is no
  remote-URL grant anywhere.
- Login happens inside each instance webview via the instance's own flows (password or
  OAuth). **Known limitation:** Google blocks OAuth inside embedded webviews; password login
  works, and for dev instances a session can be minted into Redis directly (see the
  `repo-traps` skill).
- The shell never holds instance credentials; it holds URLs.

## WebKitGTK geometry (the fragile part)

Multiwebview-per-window rides Tauri's `unstable` cargo feature, with known Linux bugs:
children are misplaced until their first `set_bounds` ([tauri#10420]) and `auto_resize`
breaks across resize cycles ([tauri#10131]). The shell therefore owns ALL geometry
(`layout.rs`): no `auto_resize`, logical units only, bounds recomputed on every
`WindowEvent::Resized` and every mode change; new instance webviews are born at their final
rect. `tauri` is exact-pinned (`=2.11.5`) in `desktop/src-tauri/Cargo.toml` — unstable
features are semver-exempt, so upgrades are deliberate.

[tauri#10420]: https://github.com/tauri-apps/tauri/issues/10420
[tauri#10131]: https://github.com/tauri-apps/tauri/issues/10131

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
# host-shared tree; the host's `tauri dev` builds into desktop/src-tauri/target,
# and two cargo builds into one target dir fight)
bun talaria box enter desktop
CARGO_TARGET_DIR=$HOME/target-desktop bun run desktop:check

# in the box — a live instance to point the shell at (seeded at box creation)
bun talaria dev          # → http://127.0.0.1:5302

# on the host — the GUI
cd ~/Development/devboxes/desktop/talaria/desktop
bun install              # shares node_modules with the box: same bun, same platform
bun run dev              # tauri dev → window on your desktop
```

In the app: *Add instance* → `http://127.0.0.1:5302`. Prove isolation with a second instance
on the same host at a different port (another box, or the primary stack) — log into both and
swap; both sessions survive.

## Gates

`bun run desktop:check` (root) = `cargo fmt --check` + `cargo clippy -D warnings` +
`cargo test` + `svelte-check`, the same list CI's `desktop` job runs. `bun run verify` is
unchanged — the desktop surface gates per-surface, exactly like `api:check`.

## Deferred on purpose

Rename/reorder instances, health badges, tray, deep links, macOS/Windows (where
`data_directory` is a no-op — macOS needs `data_store_identifier`), OAuth via external
browser, bundle/installer targets.

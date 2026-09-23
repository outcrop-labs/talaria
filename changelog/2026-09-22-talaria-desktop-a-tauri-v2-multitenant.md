- **Talaria Desktop — a Tauri v2 multitenant shell around Talaria instances
  (`desktop/`, [`docs/DESKTOP.md`](./docs/DESKTOP.md)).** One window, two
  views: the launcher's welcome screen (the only local frontend — wing mark
  on the signature dither, Mercury typography, the instance list, add), and
  the active instance's own web UI as the entire window — the interior is
  always the real UI, never a second codebase. Switching lives INSIDE the
  product: the desktop switcher (`ui/src/components/app/DesktopSwitcher.svelte`)
  wears the current instance's identity beside the logo in the nav rail and
  in the login screen's corner, rendering only inside the shell
  (`inDesktopShell()`, feature-detected — a browser gets nothing);
  Ctrl/Cmd+Shift+H opens the launcher even on instances whose deployed UI
  predates the switcher. Adding an instance validates it against the
  instance beacon (`/api/well-known/talaria-instance`) and dedupes by
  instance uuid; each instance webview gets its own data directory, so
  sessions on the same host at different ports never collide, and hidden
  webviews stay loaded (SSE survives a switch). Instance origins are granted
  exactly three commands at runtime (list / activate / show-welcome) via
  Tauri's dynamic-ACL — remote content can switch instances and nothing
  else. `decorations: false` (no GTK titlebar where the WM shows none).
  Gates: `bun run desktop:check` runs in a devbox and CI's new `desktop`
  job; the GUI runs on the host — the devbox image gained the webkit2gtk
  build deps. Verified live: devbox instance + the real hosted
  `talaria.outcroplabs.com` added through the dialog, signed in, and
  switched between via the in-UI switcher; the active instance fills the
  window at any size (an earlier persistent-sidebar design stacked the two
  webviews vertically inside WebKitGTK and drowned the app — exactly-one-
  visible-webview is the fix; manual bounds remain as belt-and-suspenders).

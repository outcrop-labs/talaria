- **Desktop notifications work in the desktop app.** The settings toggle was
  stuck off because the embedded webview reports the browser Notification
  API denied, and there is no site-settings page inside the shell to undo
  that. The shell now posts through the OS notification service — the same
  path other desktop apps use — and a click focuses the window, brings that
  instance forward, and opens the item. The toggle is the on/off switch; the
  app has to be running (quitting it stops the banners). Browser behavior is
  unchanged, including Web Push for a closed browser. Verified: the
  notification gate tests (10) and svelte-check (0 errors). A notify-rust
  post from this host reached the session bus and was refused because no
  notification service is running here (`org.freedesktop.Notifications` is
  not activatable), so a banner could not be shown locally. The desktop
  crate itself was not compiled on this host — gtk pkg-config is absent —
  so the compile is the CI desktop job.

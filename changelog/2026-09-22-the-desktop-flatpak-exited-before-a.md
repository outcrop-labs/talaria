- **The desktop Flatpak exited before a window existed.** Setup built the
  beacon client with the platform certificate verifier, which refuses to
  construct when the system store is empty — `SSL_CERT_FILE` pointing at a
  host path the sandbox cannot see, or a probe that finds nothing. That
  error aborts Tauri setup, and a `.desktop` launch has `Terminal=false`,
  so the death is silent. The client now falls back to Mozilla's roots and
  still verifies the beacon. Inside Flatpak the binary also disables
  WebKit's DMABUF renderer, which aborts on a Wayland protocol error before
  a window exists, and the launcher webview is given its bounds at the end
  of setup: the first resize can arrive before shell state exists, and a
  missed `set_bounds` leaves an undecorated Wayland window unmapped.
  Verified: the published omapak binary (`desktop-v0.6.0`) exits 101
  (`Failed to setup app: builder error`) when `SSL_CERT_FILE` names a
  missing file, and stays up after the fallback; that same binary stays up
  inside `org.gnome.Platform//50` under Xvfb; the setup layout pass reports
  1280×800 with the launcher webview present.

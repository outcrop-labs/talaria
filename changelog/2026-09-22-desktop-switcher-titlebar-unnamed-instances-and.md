- **Desktop switcher, titlebar, unnamed instances, and in-app files.** Four
  desktop-app bugs in one pass. The instance switcher in the left nav opened
  `align="right"`, so the menu painted off the left edge of the window —
  dropdowns now clamp to the viewport and the switcher always opens to the
  right of its trigger. macOS and Windows had no working titlebar
  (`decorations: false` and no custom chrome): Settings → Profile (and the
  launcher) now pick Themed / OS / None, Themed by default on every OS, with
  drag + min/max/close following traffic-light side. Switching dropped
  instances that had no company name because the row matched on a blank
  label — the switcher now matches beacon uuid then origin, and labels fall
  back to the host. Chat (and board) file chips opened `target="_blank"` on
  `/api/uploads/…`, which in the desktop webview left the app with no Save;
  every such file now opens an in-app modal (preview when we can, "cannot
  be previewed" when we cannot, Download either way, like Drive). Verified:
  `dropdownHorizStyle` and `findCurrentInstance` / `instanceDisplayLabel`
  unit tests; `sanitizeFilename`; cargo tests for settings default/roundtrip.

### Added

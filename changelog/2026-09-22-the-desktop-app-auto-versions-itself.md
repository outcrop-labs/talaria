- **The desktop app auto-versions itself off main.** Every green build of a
  push that touches `desktop/` now mints the next minor version — highest
  suffix-free X.Y.Z across the `v*` and `desktop-v*` tags, minor+1 — and
  publishes it as `desktop-vX.Y.0`: a regular GitHub Release carrying all
  platform installers, `SHA256SUMS`, update signatures, and `latest.json`, so
  `/releases/latest` resolves to it and installed desktop apps auto-update
  (the feed flips only after every asset is uploaded). Until now trunk desktop
  builds were `0.0.0-sha…` artifacts that expired. Majors stay manual
  (dispatch `desktop-package` with `version=1.0.0, publish=true`); auto never
  crosses a major, and stable `vX.Y.Z` cuts raise the baseline. Verified:
  `bun run check`; the merge itself is the first live mint (resolve →
  desktop-vX.Y.0 → release + latest.json flipped) per RELEASING.md's new
  auto-minor section.

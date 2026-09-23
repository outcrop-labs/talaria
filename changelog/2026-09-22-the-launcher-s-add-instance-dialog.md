- **The launcher's add-instance dialog rendered behind the content that
  opened it** — the welcome content sits at `z-10` and the dialog had no
  z-index. Both overlays are `z-50` now.

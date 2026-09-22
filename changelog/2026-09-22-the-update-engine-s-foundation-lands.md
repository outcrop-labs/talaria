- **The update engine's foundation lands — dormant by design.** `api/src/update/`
  is the groundwork for the app rolling its own container: install modes
  (image / checkout / dev / off — only an image install can ever roll, and
  the image's own env now carries the signal), the layout of the
  updater-owned compose project (slot names, env files, the pinned traefik
  edge), the persisted state row (auto-update off by default, digest pins,
  capped run history — corrupt rows fall back to the safe default), and the
  registry reader that resolves the moving `main` tag to a digest the way
  the distribution spec says (anonymous bearer token, manifest HEAD,
  index-walked version label) — proven live against ghcr. Nothing routes to
  any of it yet: no behavior changes for any running install in this drop.

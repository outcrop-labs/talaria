- **Stale `/knowledge/<docId>` links open the document.** The #316 href
  repair moved every retrieved-doc writer to the `?doc=` permalink — but the
  copies already out in the world (hrefs baked into notification emails,
  Qdrant payloads until the next reindex regenerates them) still arrive as
  the one-segment form, which the route parses as a *space*: the roster
  holds no such space, the view falls to the first space's overview, and the
  click that promised a document quietly opens a folder — the same quiet
  wrong answer the `?r=` net already casts for research. Knowledge now
  resolves it the way `?doc=` resolves: a path segment that names no space
  in the loaded roster but names a document is that stale shape, and it
  redirects (replace) to the document's own space with the document open.
  A segment that names neither stays on the overview as before, and the
  selection memory does not record the pending segment as a space.

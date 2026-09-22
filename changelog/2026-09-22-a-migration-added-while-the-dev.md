- **A migration added while the dev server runs now applies on the next
  request.** The migration runner caches its "done" promise on globalThis so
  vite's SSR module reloads don't re-open the pool — but that also meant a
  MIGRATIONS array that grew after boot was never re-run, and every query
  touching the new column 500'd ("column does not exist") until someone
  restarted the dev server, which is exactly how `preferred_effort` got stuck.
  The runner now records the array length of the last successful run beside
  the promise, and a GROWN array re-arms the run: already-applied statements
  no-op against schema_migrations, appended ones apply, and edits to applied
  statements still trip the checksum check. Production is unaffected — the
  array never changes inside a running process there.

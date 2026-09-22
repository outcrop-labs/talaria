- **One dither engine for ui + desktop (−645 lines).** `desktop/src/lib/dither.ts`
  was a 649-line copy of ui's, kept in step by a comment that said so. The
  engine is `ui/src/lib/dither-engine.ts` now — it imports NOTHING (no `@/`
  alias, no framework, a local `clamp01`), which is what lets a separate package
  share it; `ui/src/lib/dither.ts` stays as the door its ~15 importers already
  point at (`export * from './dither-engine'`). `desktop/vite.config.ts` aliases
  `@dither` onto that file and allows the cross-package read with
  `server.fs.allow: ['..']`. `docs/DESKTOP.md` stops describing a port. The
  per-side `.svelte` wrappers stay per-side — ui's `DitherLayer` (219) and
  `WingMark` (38) are not desktop's (84 / 55), and only the engine was ever
  identical.

  Verified: `bun run typecheck` (0 errors), `bun run build` (ui), and
  `cd desktop && bun run build:vite` — 120 modules including the aliased engine
  (the built bundle contains `hash01`, so the shared file really is in it).
  `bun run desktop:check` **cannot** run on this host (its Tauri build needs
  `dbus-devel`, and cargo panics in libdbus-sys's build script — pre-existing,
  unrelated to this change). The launcher's rendered field is likewise not
  visually verifiable here (no display, no docker): `docs/DESKTOP.md`'s own
  rule, boxes build and the host runs.

- **Desktop's typechecker can see the dither engine — CI on main goes green.**
  The dither merge (#410) taught `desktop/vite.config.ts` an `@dither` alias
  into `ui/src/lib/dither-engine.ts`, but a vite-only alias is invisible to
  svelte-check, and `vite.config.ts`'s `node:url` import had no `@types/node`
  behind it: the desktop CI job came up red on main with 3
  module-resolution errors while the desktop-package build stayed green —
  vite resolves the alias; the type checker couldn't. `desktop/tsconfig.json`
  now maps `@dither` in `paths` (same target as the vite alias) and lists the
  `node` types; `@types/node` joins devDependencies at ui's `^22.10.0`.
  Verified: desktop `bun run typecheck` 0 errors (the engine is now part of
  desktop's checked program), `bun run check` green.

- **A new invariant keeps `@types/*` packages out of runtime dependency
  sections.** `scripts/check-invariants.mjs` now fails on any tracked
  `package.json` that lists a `@types/` package under `dependencies`,
  `optionalDependencies`, or `peerDependencies` (id
  `types-package-in-runtime-dependencies`). Type stubs are compile-time only —
  in a runtime section they ride production installs and ship to every deploy
  for nothing, which is exactly how `@types/nodemailer` ended up in
  `ui/package.json` dependencies until GH #264 pulled it out by hand. The
  check discovers manifests via `git ls-files` (gitignored client subrepos
  under `apps/` stay out of scope) and teaches the devDependencies move in the
  failure; a believed-legitimate exception is an argument in the PR, not a
  widened rule. Verified: clean tree passes, moving `@types/nodemailer` into
  ui dependencies fails with the new id, `git checkout` restore is clean, and
  `bun run check` passes.

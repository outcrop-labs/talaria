- **The publish budgets match the build they pay for.** `api-package`'s
  30-minute job limit (and `app-image`'s 30-minute digest poll) were sized
  against a build that never happened: cargo-chef's skeleton compiled in 3-6
  seconds, so the job only ever paid for the cook layer. With the stub gate in
  place the `build` stage does the real release compile of the workspace —
  09-21's first honest run reached `Compiling talaria-api v0.1.0` at 1,470s and
  the limit cancelled it 30s later, so nothing published, and `app-image`'s pin
  poll (60 × 30s) gave up on a digest that was still building. Now: 60 minutes
  for the package job, 70 for the pin job, 100 × 30s for its poll — enough for
  a cold cook (~8min) plus that compile plus the static musl link, with room
  left over.

  Verified: `bun run check`; the next api-touching push (the merge of this
  file is one — both workflow files are in their own `paths` filters) exercises
  the budget end to end and publishes `sha-<sha12>` + `main` for both images.

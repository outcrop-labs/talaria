- **The Work view had never rendered, and a getter is what hid it.** `/work`
  threw on every mount from the day the skeleton landed (`b8e365b3`, #495) —
  "can't access lexical declaration 'm' before initialization", caught by
  `ErrorFallback` so the surface reported a render failure rather than a blank
  page. The binding is `selectedSessionId`, and `'m'` is only what the minifier
  called it.

  `selectedSessionId` was declared at line 113 and first read at line 99, inside
  the `() => selectedSessionId` getter handed to `useConversationMembers`. A
  getter defers the READ, not the binding — and this one is not deferred:
  `createQuery` resolves its options function **eagerly** to seed the observer
  (`createBaseQuery.svelte.js`, `new Observer(client, resolvedOptions)`, carrying
  its own `svelte-ignore state_referenced_locally — intentional, initial value`).
  So the getter ran during the component's init, fourteen lines before the
  `const` it reads existed, and the read landed in its own temporal dead zone.
  Moving the declaration above its first reader is the whole fix.

  **`Plan.svelte` is the proof and the reason this is a one-line class.** The
  Work view inherits the Plan idiom wholesale, and Plan carries these same two
  lines in the opposite order — the `$derived` at 84, `useConversationMembers`
  at 92 — which is exactly why Plan has always worked. The skeleton copied the
  idiom and landed the declaration in the wrong place; nothing about the pattern
  is wrong, only the position, which is why the fix now reads as a comment
  explaining that the position is load-bearing.

  How it shipped is worth recording, because no gate in the tree could have
  caught it. #495's verification line reads "`bun run check` green, typecheck 0
  errors, 1332 ui tests, cargo fmt clean, migrations replay 395/395" — and not
  one of those mounts a component. `svelte-check` cannot see a TDZ: the binding
  is declared, is in scope, and has the right type at both lines; only the
  runtime evaluation order is wrong. Vite's dev server hides nothing here either
  — the throw is unconditional in every build — so the view was simply never
  opened in a browser. The repo already says this out loud ("Typecheck proves
  nothing about behavior. Drive the real surface"); this is what skipping it
  costs: three further commits (#501, #507, #512) built on a view that could not
  render, and the instance ran it broken for five days.

  Verified: the defect reproduced byte-for-byte before fixing it — a build of
  the deployed revision `cedd5cea` emits `Work-uTOaLehs.js`, the exact chunk
  filename (a content hash) the running instance serves, and an unminified build
  of that tree shows `membersQuery` at line 383 and `const selectedSessionId` at
  392. After the fix the same build puts the declaration at 386 and its reader
  at 387. Rollup's own module graph was queried directly for cycles rather than
  trusted to the build log (Vite silences `CIRCULAR_DEPENDENCY`): 48 across 5,378
  modules, 47 inside `svelte/src/internal` and one benign self-import in
  `KbDocRow.svelte`, none reachable from `/work` — this was never a circular
  import. `bun run check` clean; `bun run typecheck` 0 errors.

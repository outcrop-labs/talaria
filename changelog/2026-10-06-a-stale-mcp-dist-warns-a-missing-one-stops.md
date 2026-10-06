- **A stale `mcp/dist` warns; a missing one still stops the stack.** `talaria
  dev` rebuilds the agent toolkit when `mcp/src` is newer than `mcp/dist`, and a
  failed rebuild used to kill the whole bring-up with one message that conflated
  two different failures. They are not the same: a dist that exists and is
  merely out of date **still serves** — the api spawns it, agents get the
  previous build's tools, and the worst case is tool descriptions that lag
  `mcp/src`. That is annoying, not fatal, and not a reason to refuse to bring up
  a stack somebody is using to work on something else. A dist that is **absent**
  serves nothing, so a stack that came up anyway would hand every agent an empty
  tool list and look like a product bug. So: missing dies, stale warns and
  carries on, and each says which case it is.

  **The CLI's fake `ctx.plant` can now scope an answer to a `cwd`**, which is
  what made the old behaviour hard to test honestly. `talaria dev` runs
  `bun run build` in two places — `mcp/` and `omp-auth/` — and `plant` keyed
  only on `(cmd, args)`, so a test that wanted to fail one of them failed both.
  The omp-auth bridge's "a failed build warns and dev carries on" test worked
  around that by arranging mtimes so the toolkit's build would not run at all.
  That arrangement does not survive a fresh checkout: CI gives `mcp/src` and
  `mcp/dist` the same checkout time, the toolkit built too, hit the planted
  failure, and died before the bridge was ever reached — so the test failed on
  CI while passing locally. It now names the directory it means.

  Verified: `bun test` in `cli` 222 passed (was 220) — the two new cases cover
  each path, and the stale one is a real gate: forcing the `die` branch
  unconditionally makes it fail with the missing-dist message, which is how the
  old behaviour read. The omp-auth test now passes for its own reason rather
  than by mtime coincidence.

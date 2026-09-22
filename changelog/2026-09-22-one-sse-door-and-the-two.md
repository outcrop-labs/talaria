- **One SSE door, and the two rules that keep it.**
  `ui/src/lib/sse.ts`'s `openStream(url, onMessage)` now opens every EventSource
  in the client: the board rail (`boards.svelte.ts`), the channel rail
  (`channels.svelte.ts`), the `/api/me/events` fan
  (`user-events.svelte.ts` — it keeps its reference-counted subscriber registry
  on top) and `WorkWatch.svelte` (which keeps its own frame parsing). What the
  four had disagreed about is the part that breaks: a frame that does not parse.
  Two swallowed it, one let `JSON.parse` throw inside the listener (an unhandled
  error with the stream still open), one returned early. The door hands the raw
  data over — that judgment is per-surface, and every consumer here has a poll
  floor behind it — and owns the connection and its teardown, which is what the
  four `es.close()` calls could each forget. `ts-event-source-outside-the-door`
  and `ts-maybe-getter-copy` are the wave's last two rules; **15 rules, all
  clean**, `0` duplicate clusters in both languages, `DUPLICATE_BODY_ALLOW`
  empty.

  Verified in the running app (docker + the dev stack are up now, so this is
  real exercise, not inference): the seeded board rendered, and a ticket created
  out of band through the API appeared in the open board view **without a
  reload** (3 → 4 tickets) — which is the board rail's `openStream` invalidating
  through the refactor. No console errors. `bun run verify` green (1,186 tests).

  Also recorded here: **the `#[ignore]`d live-DB suite now runs and is not
  green, and none of its failures is this sweep's.** With the schema migrated
  (`ui/.env` + `bun -e 'migrate()'`), 67+ live tests pass — including every test
  the W2 test-support module touched. Four fail: `attribution::a_live_turn_
  outranks_the_hirer` (A/B-proven pre-existing: nothing in the tree ever installs
  `talaria_attribution::CONVERSATION_OWNER`, so that rung of the ladder is dead
  in production too), `llm_models::minted_key_lists_the_catalog` (A/B-proven
  pre-existing), `realtime_fan::the_fans_reach_exactly_their_audiences` (an
  ordering flake: the assertion compares the same two ids in arrival order,
  ~1 run in 3), and `workchains_live`'s three (also flaky run-to-run, in a file
  the sweep never touched). A/B = the same test run from the pre-sweep commit.

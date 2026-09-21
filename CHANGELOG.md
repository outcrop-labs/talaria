# Changelog

All notable changes to Talaria. Milestone labels refer to the historical plan, [`docs/history/PLAN-design-notes.md`](./docs/history/PLAN-design-notes.md).

## [Unreleased]

- **The dither field repaints what changed, not the whole field — the desktop
  shell's launcher stops being a furnace.** `ui/src/lib/dither-engine.ts`'s frame
  re-evaluated and re-drew EVERY cell of every mount, every tick. At the house
  grain (pitch 2 / dot 1) the launcher's full-window field is 247,040 cells, and
  keeping it shimmering cost 575ms per paint on average (633ms worst): the page
  ran at 1.5 fps with 84% of the main thread inside long tasks — and the launcher
  stays mounted (and painting) behind an active instance. The field is a function
  of the sources' GEOMETRY, so it is now computed once per change
  (`density`/`ink`, per cell) and a shimmer tick re-rolls only what the shimmer
  can change: the threshold, for the cells whose lit state can flip
  (`|density − threshold| ≤ shimmer/2`). Every other cell is left as it is, cell
  by cell, against a `painted` buffer in the canvas's own 8-bit resolution, and a
  colour string is built once per distinct pixel instead of once per cell. A LIVE
  frame (a tween in flight, a travelling wave) still re-evaluates and redraws, so
  transitions keep full rate. Two deliberate consequences: the ALPHA no longer
  rides the shimmer jitter (it is a function of density alone — a jitter that
  moved every cell's alpha by a level made every pixel differ on every tick; the
  visible sparkle, the lit/unlit step, is unchanged), and a field nobody can see
  stops painting — an IntersectionObserver on the canvas plus `visibilitychange`
  park the loop, and becoming visible repaints from a clean canvas.

  Measured in a worktree stack (its own DB/Redis, app :5302; the launcher page on
  :5290), same method before → after: the launcher field 575ms → 7ms per paint
  (633ms → 17ms worst), 13 long tasks and 7.6s of blocking in 8s → 0 and 0, 1.5 →
  60 fps, main thread 100% → 7.5% busy; the inbox's full-pane empty-state field
  85ms → 2ms per paint, 50 long tasks and 4.6s → 0, 34 → 60 fps, 62% → 5.6%.
  Parity proved rather than assumed: the new paint path rendered against the
  previous loop verbatim into a second canvas differs in **0 pixels** for three
  source mixes (edges+organic, rect+halo+ramp, cover mode) and for a masked
  field; the shimmer rule was checked over 525,525 (density, threshold, jitter)
  triples with **0** cases where the lit/unlit decision differs from the
  density-jitter it replaces. Five routes (inbox, brief, boards, knowledge,
  artifacts) load with no console errors, fields painted, 0 long tasks after
  settle. The desktop GUI itself was not run — this host has no display — so the
  launcher was exercised as the page it is, in a browser. `bun run check` and
  `bun run verify` green.

- **W8 (part 1): one stale-board banner, one page skeleton.** The board's three
  lenses each wrote the same "a reference read failed, the tickets did not"
  banner — list, kanban and gantt agreed on the shape and disagreed, twice, on
  the sentence. `components/board/StaleBoardNotice.svelte` is that banner: it
  takes the statuses read, an optional labels read, the word the lens uses
  ("Columns" in kanban) and, for gantt, its own sentence for "the status set
  never arrived"; everything else — which read is blamed, what the retry
  refetches, and the rule that a failed labels read only costs tint — is in one
  place. And `routes/app/ArtifactPageSkeleton.svelte` is gone: it was
  `KbDocPageSkeleton` with the prose widths frozen, and only the widths had
  drifted (58% where knowledge's list lands 88% on the tenth bar).
  `ArtifactEditor` renders `<KbDocPageSkeleton bars={10} />` — knowledge's
  widths, deliberately.

  Verified in the running app, not inferred: with `/api/boards/<id>/labels`
  aborted, the extracted banner renders "Could not load labels…" **and the board
  still draws its 5 tickets across 6 columns** (the mark-don't-replace rule); and
  the artifact editor's cold load renders exactly **10** prose bars with the
  detail read delayed, which is the geometry the deleted skeleton had.
  `bun run verify` green (1,186 tests), `bun run check` green (15 rules, 0
  duplicate clusters).

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

- **Consistency pins that ride along.** The Rust crates stop re-spelling
  ladders the constant crate already owns: `talaria-fitness-talaria-tools`
  (which had its own `PRIORITIES`/`EFFORTS`/`COLORS`) and
  `talaria-routes-comms`'s `channels_id_plan` now import
  `talaria-task-const`'s, which is what the api validates against. The
  cross-language pin in `ui/src/lib/task-const.test.ts` grows from one
  assertion to four: the priority ladder, the effort ladder, `talaria-statuses`'
  off-board list (as before) and mcp's `AGENT_STATUSES` — each read out of the
  other language's source, so a drift on either side fails the client's suite.

  `scripts/check-invariants.mjs` gains the app-DB container-name pin: the
  resident tier (`ui/src/server/app-db.ts`) and the CLI's backup
  (`cli/src/cmd/backup.ts`) both compose `talaria-appdb-<instance>-<slug>`, and
  a drift would surface as "that container is not there" on the day somebody
  needs the dump. It is a pin, not an import — one end reads `process.env` and
  the other a passed-in record — so it checks the shape: the prefix, both
  variables, the `'talaria'` fallback.

  **Not done**: the ports pin (it needs `cli/src/ports.ts`, which is W12's), and
  `talaria-fitness-world`'s bare `INBOX`/`ASSIGNED`/… consts — there is no
  home to import them from: the board statuses are DB columns with a virtual
  default list, not constants, so the crate's copy is its own wire vocabulary.

  Verified: `bun run check` (13 rules, 0 clusters), `bun run api:check`,
  `bun run verify`.

- **UI data layer: one home per helper (part 1).** The sweep's UI wave, done as
  the pieces that are verifiable without a browser:

  * **The three duplicate function bodies the detector was holding open for the
    UI are gone — the allow list is now EMPTY, and `bun run check` reports zero
    clusters in both languages.** `lib/list-nav.ts` owns the
    Arrow/Enter/Tab grammar (EmojiList + MentionList; each menu's exported
    `onKeyDown` is an adapter over a `createListNav` bound to its own runes),
    `lib/outside-click.ts` owns the outside-pointer test (Popover +
    DropdownMenu), `lib/menu-position.ts` owns the caret-anchored placement
    (mention-suggest + slash-commands) — which is also where the two caret
    menus' flip-above rule finally agrees.
  * **`lib/reactive-arg.ts`** — `MaybeGetter<T>` and `resolve()`, declared
    locally in sixteen files (plus two one-off spellings, `resolveModel` and
    `resolveValue`). One type, one resolution.
  * **`toastError(title, e)`** — 93 call sites wrote
    `pushToast({ title, body: errorMessage(e), tone: 'danger' })` by hand; the
    helper is that pairing, and 54 files call it now. Four sites whose body is a
    composed sentence (`\`${label}: ${errorMessage(e)}\``) keep their own call —
    the rule is the pairing, not the tone.

  **Not done in this wave** (and why): `persist.ts`, `statuses.ts`'s
  `STATUS_COLOR`, `format.ts`'s usd/bytes/token/duration spellings, the
  `teams.ts` directory hook, the context-menu/SortHeader helpers and
  `sse.ts`'s `openStream`. Each is a behavioural refactor of a component's
  render path, and this host has **no docker, so no dev stack and no browser** —
  the remaining UI waves (W8–W11: QueryState adoption, the component-pair
  merges, the row-chrome and composer-picker collapses) would be landed
  unexercised and pixel-unverifiable, which is the one thing their wave notes
  say not to do. They are left for a session that can run `bun talaria dev`.

  Verified: `bun run verify` (check + svelte-check 0 errors + 1,183 vitest
  tests).

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

- **One client for `/api/skills` — and the stale list it caused.** Two hooks
  backed one endpoint: `lib/skills.ts`'s `useSkills()` under key `['skills']`,
  and `lib/workflows.ts`'s `useSkillLibrary()` under `['skill-library']`. Each
  surface invalidated only its own key, so renaming or deleting a skill from the
  Studio left the Skills library showing the old name until a manual refresh.
  `workflows.ts`'s second client is gone; `Studio.svelte`, `StudioGuide.svelte`,
  `WorkflowDetail.svelte` and `SkillRow.svelte` now read `useSkills()` and
  invalidate `SKILLS_KEY`, so any surface's write updates all of them.
  `SkillOwner` carries the `label`/`model` the Studio rows read.
  Verified: `bun run verify` (svelte-check 0 errors, 1,183 tests). The rename
  round-trip itself was **not** exercised in a browser — no docker on this host,
  so the dev stack cannot be started; the fix is the shared key, which is what
  `SkillEditor`/`SkillsLibrary` already invalidate.

- **Dead code deleted across the UI, cli and their docs (~440 lines, no
  behaviour).** Every one was verified by grep before it went: no importer, no
  invariant anchor, no test.

  * `ui/src/server/permissions.ts` (192 lines) and its 214-line test — the TS
    permission catalog and `requirePerm` gate. The live catalog is
    `api/crates/talaria-permissions/src/lib.rs` now; the file survived only as a
    re-export in `api-guard.ts` nothing imported. `docs/PERMISSIONS.md` and
    `docs/API-CONVENTIONS.md` stop calling it "the catalog" and name the Rust
    twin of each guard.
  * `ui/src/lib/notify-classes.ts` — the routing/digest half
    (`isNotifyRoute`, `isNotifyClass`, `KIND_CLASS`, `notifyClassOf`,
    `resolveNotifyPrefs`, `DIGEST_PREF_KEY`, `storedDigestPref`,
    `digestEnabled`): zero importers; `talaria-notify` owns the vocabulary and
    the derived answer. What stays is what the client draws from —
    `NotifyClass`, `NotifyRoute`, `NOTIFY_CLASSES`, `NotifyPrefs`,
    `DigestPref`. (238 → 151 lines.)
  * `TERMINAL_KINDS` / `isTerminal` in `daily-brief-types.ts` and
    `RawFocusItem` in `inbox-focus-types.ts`. **Not** the two exported arrays
    the plan named: `BRIEF_SECTIONS` and `BRIEF_ENTRY_KINDS` are the *source* of
    `BriefSection` / `BriefEntryKind` (`typeof X[number]`), so they are
    load-bearing, not dead.
  * `cli/src/paths.ts`'s `isNewer` and its `boxDir(devboxes, name)` — the
    latter shadowed by `cli/src/cmd/box/shared.ts`'s `boxDir(ctx, name)`, which
    is what every caller imports.

  Verified: `bun run verify` (check + svelte-check 0 errors + 1,183 vitest tests
  pass), `bun talaria --help` still renders. No running-app pass: this host has
  no docker, so the dev stack (and therefore any visual check) is unavailable.

- **One home per duplicated helper in the api crate.** Every cluster the new
  duplicate-body detector reported is gone (`0 clusters`, down from 13), and the
  named families the sweep listed followed:

  * `audience` → `talaria-runs-define` (beside the `Authority` it answers with;
    research-def, runs-agent-hire and runs-plan-draft each had a copy).
  * `roll_drain_ms` → `talaria-update-layout` (already the import home for the
    two update crates; fleet-reconcile now reads it there).
  * `assistant_owner_for`, `personal_assistant_owners` → `talaria-users`;
    `has_oauth_tokens` → `talaria-mcp-oauth`; `talaria-mcp/src/registry.rs` had
    private copies of all three.
  * `percent_encode`/`percent_decode` → `talaria-body`. Four crates wrote JS's
    `encodeURIComponent` (gateway provider URLs, google-client path segments,
    session cookies, inbox cursors) and two wrote the decoder; the unreserved
    set is a JS contract, and five of them imported it from each other in a
    chain. HEX_UPPER is a table now, not `format!` per byte.
  * `now_ms`/`now_iso` → `talaria-agent-auth` (7 `u64` copies + `now_iso` ×3;
    the clock crate already owned the `i64` one and `epoch_ms_to_iso`).
  * `hex` → `talaria-body` (4 copies: LLM key minting, update signing, SigV4's
    canonical request, skill hashing).

  The detector's allow list is down to the three TypeScript clusters the UI
  waves own; each Rust entry died exactly when its work landed, which is what
  the stale-entry check is for. The plan's remaining W3 rows (`NowFn`,
  `utf16_*`, `fold_slug`, `truncate_bytes`, the `js_*` coercions, `sha256_hex`)
  are **not** in this commit: they are either type aliases or pairs whose
  signatures differ, so they need per-call-site surgery rather than a
  collapse — and none of them is detector-flagged, so nothing regrows
  unwatched. (Honest note: the sweep's "~800 lines" for this wave is really
  ~150 — most of the duplicates it named were already collapsed by W1/W2 or
  were never byte-identical.)

  Verified: `bun run api:check` (fmt + clippy `-D warnings` + `cargo test
  --workspace`), `bun run check` (13 rules, `gen-docs --check` clean).

- **Handlers answer `Result<Response, Response>`, and the gate/body/secretbox
  unwraps collapse onto one call each.** Five passes over the route crates,
  every one locked by a new invariant rule:

  * **Gates propagate with `?`.** ~380 call sites spelled out
    `match require_user(&state, &headers).await { Ok(u) => u, Err(gate) => return
    gate }`. Every guard already returns `Result<_, Response>`, and axum
    implements `Handler` for any `R: IntoResponse` — `Result<Response, Response>`
    is one — so the whole match becomes `require_user(&state, &headers).await?`.
    `rust-hand-wrapped-gate` fails the next one.
  * **`object_or_400`.** 194 route files hand-wrote the same 400 for a
    non-object body. `let obj = object_or_400(&parsed)?;` is that conversion,
    defined beside the envelope it produces (`talaria_body` stays pure and
    answers the message only). `rust-as-object-unwrap` fails the next one.
  * **`secretbox_or_500`.** 29 `state.secretbox()` hand-unwraps become
    `secretbox_or_500(&state, "<context>").await?`. It lives in `talaria-session`
    rather than `talaria-error` — `AppState` depends on the error crate, so the
    cycle the plan assumed away is real; session is the layer that may name both
    `AppState` AND a `Response`. `rust-secretbox-unwrap` fails the next one.
  * **In-crate gates get one home each.** `channel_gate` (6 copies in comms),
    `edit_gate` (boards), `owner_gate` (teams) and `can_manage_agent` (fleet) now
    live in their module's `mod.rs`; `talaria_params::uuid_gate` absorbed its 4
    copies and gained `uuid_gate_404` for the one route that answers "not found".
  * **`api/tests/support/`.** An integration test is its own crate, so the
    shared fixtures HAD to be copied per binary — 19 `pool()`/`pg()`, 5
    `fabricate_user`, 3 `person`, 3 `sweep_user_rows`, 3 `app_state` and the
    same DATABASE_URL expect string in 25 files. The prefix variation is a
    parameter now. (The 13 `cleanup` helpers stayed put: each holds a different
    table and WHERE clause, so they were never copies.)

  **The docs oracle moved one row group, on purpose.** `scripts/gen-docs.mjs`
  now follows helpers in the module's `mod.rs` (the gate move would otherwise
  drop every 403 from the reference) and reads the new `object_or_400` binding.
  That surfaced a pre-existing inaccuracy it had never been able to see: the
  three `/api/teams/{id}*` GET rows are `session + view:/teams`, because
  `reader_gate` consults `require_view` for a non-member. `docs/api/**` is
  regenerated with exactly those 6 rows changed; every path, method, body field,
  return shape and status list is byte-identical.

  Verified: `bun run api:check` (fmt + clippy `-D warnings` + `cargo test
  --workspace`, exit 0), `bun run check` (13 rules clean, `gen-docs --check`:
  245 routes, no further drift). The `#[ignore]`d live-DB suite was **not**
  executed — this host has no Postgres, Redis or docker (the dev stack cannot be
  started here), so `api/tests/support/` is compile- and clippy-verified only.

- **One internal-error trap: ~860 hand-written log-and-500 sites collapse onto
  `talaria_error::internal`.** The `tracing::error!("…: {e}"); return
  thrown_internal_error();` pair was written out by hand at every call site that
  did not catch an engine error, in five spellings (terminal `return`, tail
  expression, `Err(…)`, `Some(…)`, fully-qualified). Because each site re-made
  the one decision in it, about a quarter re-made it as "no log at all" — a 500
  with nothing in the log and no way to tell which read failed. `internal`
  (api/crates/talaria-error/src/lib.rs) is that pair once: it logs
  `"{context}: {e}"` and returns the same byte-exact `thrown_internal_error()`
  response. Rust `tracing::error!` in api/crates: 993 → 172; `return
  thrown_internal_error();` → 1 (inside `internal`). Two new invariant rules
  (`rust-trap-block`, `rust-thrown-internal-error-outside-its-envelope`) fail the
  next copy, and the checker now scans `api/crates` as a second source tree
  (`scripts/check-invariants.mjs`, `lang: 'rust'`; a duplicate-function-body
  detector with a name+path allow list rides along). Non-conforming sites — a
  log line quoting a value rather than the error, or an `.is_err()` that logged
  nothing — were converted by hand; those log lines gain a `: <detail>` suffix.
  Wire bytes, status codes and the route table are unchanged. Verified:
  `bun run api:check` (fmt + clippy `-D warnings` + `cargo test --workspace`,
  exit 0), `bun run check` (gen-docs `--check`: 245 routes, 25 files, no drift).

- **Routes partition: 7 group crates + a facades crate.** The 98-second
 `talaria-api-routes` unit becomes seven parallel crates; cold build
 207.3s → 183s (now faster than the pre-split monolith's 186s), routes
 edit rebuild 8.5s → 4.8s. Also repairs two latent split casualties the
 fail-fast test runner had been hiding: the toolbox census test lost its
 registry scan (moved to harness-defs next to the registry), and
 hermes-skills read `scripts/skills` off a stale relative path. The full
 suite is 2,175 tests — the earlier "868" figure was an undercount from
 aborted runs. Verified: `cargo test --workspace` (2,175 passed, exit 0),
 `clippy -D warnings`, `bun run check` (245-route table unchanged).

 117 static-init) is safe-by-construction — constant regexes, guarded
 doubles; the two fragile guarded-unwrap shapes (history `kind`, oauth
 callback tuple) are destructured instead. The workspace's one remaining
 `unsafe` (SSE metered stream `get_unchecked_mut`) is audited sound.
 Split-era helper copies deduped onto their owners. Verified:
 `cargo clippy --workspace --all-targets -- -D warnings`,
 `cargo test --workspace` (868 passed).

 mid-build at 15. Verified: the rerun on this change is the first to fit
 the budget cold.

 it. Verified: `bun run check` (doc links).

 build is cargo-chef-layered (deps build once per manifest change) and
 exports its layers through a buildx registry cache (`:buildcache`, mode=max)
 — the gha backend can't be used because release.yml calls the build via
 workflow_call. Verified: `bun run check`; the image build itself exercises
 on the next push to `main` (no docker on this box).


  the split: clippy `-D warnings` and 868 tests green. Verified:
  `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace`.





























































































- **API dev builds: opt-level 1 + mold.** `api/Cargo.toml` `[profile.dev]`
  compiles our crate at `-C opt-level=1`, deps at 3, `debug =
  "line-tables-only"` (unwind stays — tests and catch-panic). gnu linux
  links with mold (`api/.cargo/config.toml`); CI installs the package, the
  devbox image does too. musl/package image unchanged until mold-on-musl is
  proven. Verified: `cargo build` after the profile flip (4m 34s, deps at
  opt-level 3); incremental `touch src/routes/mod.rs` 20.7s (was 23.7s);
  `cargo clippy --all-targets -- -D warnings` green; `cargo test` green;
  `bun run check`.


- **Workbench agents pack against the docker host, not a 3-job stall.**
  Admit uses `docker info` MemTotal when the API is cgrouped smaller than
  the VM (`/proc/meminfo` when they match). Keep-back is 25% of that total
  (2–16 GiB). Ceiling is total − keep-back (never above 32 GiB);
  `oom_score_adj: 500`. Reservation sums each live job's effort (1/2/4 GiB).
  A refused start writes `work_wait` and the ticket API returns `{ session,
  wait }` — board cards and the ticker show queued position + reason; wait
  rows clear only when a session is actually live. Verified: `merge_host_mem`
  + `jobs_bytes` + `work_wait::wire` tests; `bun run check`.

- **Migrations: CI upgrade baseline back to `origin/main`.** The repair
  (#398) pinned the upgrade pass's baseline to e9f08476 — the last array
  any deployed instance ran — because main's tip then carried the broken
  order. With the repair merged, the default (`origin/main`) is the honest
  baseline again and the pin is gone. Verified: this PR's own migrations
  run compares origin/main's array against itself — `applied: 0`, snapshot
  matches.

- **Migrations: repair the mid-array workchains insert + CI now replays the
  upgrade path.** #394 landed its three workchains statements in the MIDDLE
  of the append-only array; every fresh database (all of CI) applied it
  green, but every deployed instance refused to boot on the roll — the
  checksum guard fired `migration 338 changed after it was applied` and the
  canary (outcrop) came back rolled to e9f08476 within minutes, public
  domain 200 again. Fixed: the workchains statements moved to the END of
  the array (fleet ledgers at 343 rows upgrade cleanly to 348). CI: the
  migrations job now also replays the UPGRADE — baseline array into a
  second scratch postgres, then the PR's array on top of that ledger, plus
  the snapshot check on the upgraded database — which fails exactly where
  the fleet failed (verified by replaying the incident locally: baseline
  343 → broken array → guard, exit 1; baseline 343 → fixed array → applied
  5, total 348, snapshot matches). Dev stacks that ran the broken array
  from a workchains-era branch need a `talaria reset` — no deployed
  database ever applied it. The upgrade baseline is pinned to e9f08476
  (the last array any instance ran) until the first post-repair migrations
  PR flips it back to origin/main.

- **Workchains: CI repair after the main merge.** The merged tree failed
  the api and migrations CI jobs. Fixed: regenerated
  `ui/src/server/db/schema.snapshot.sql` (the merge hand-carried a stale
  snapshot — table ordering and `"position"` quoting drifted from what
  pg_dump emits); dropped the dead `read_gate` helper and an unused
  parameter that tripped clippy `-D warnings`; repaired
  `tests/workchains_live.rs`, which never compiled (missing `pg`
  bindings, `as_deref` on tuple options, moved `String`s); and updated
  the two unit tests that pinned the pre-workchain worlds (the views
  enum without `workchains`, the digest derivation without the
  `workchain_turn`/`workchain_paused` classes). Verified: `bun run
  api:check` green (fmt, clippy `-D warnings`, 2067 tests), two-pass
  `migrations:check` against a scratch postgres 16 (`applied: 0` on the
  second pass, snapshot matches), and full `bun run verify` green
  (svelte-check 0 errors, 1200 tests).

- **Workchains: the ticket's own chain section.** A ticket in a chain
  shows a Workchain block in its detail view — chain name, `step N of
  M`, and (for editors) move-earlier/move-later/leave verbs. The move
  ships the full reorder payload (`moveStepOrder`, unit-tested: swaps,
  end refusals, unknown ids, no caller-mutation); leaving unlinks the
  ticket and the chain reads on. Joining stays in the Workchains view's
  pickers; step-assignee edits are deliberately absent (a step's
  assignee is the ticket's assignee — one edit surface, and the MCP
  guardrails hold by construction: no agent tool reaches the
  workchain routes). Verified: `bun run verify` green (1179 tests);
  the section driven in headless Chrome — renders for a chained
  ticket, correct position read (screenshot filed with the ticket).

- **Workchains: the fourth Boards view.** The view toggle gains a
  `workchains` lens (`?view=workchains`, URL-driven like the others,
  saveable as a saved view, persisted per board in localStorage). Each
  workchain renders as a horizontal rail of compact step cards — status
  dot, ref, title, effort, due, assignee avatars — joined by chevrons:
  done steps fade under a check, the head wears the accent ring and a
  `HEAD` marker, waiting stays quiet, archived steps strike through and
  always render (they are chain structure, not filter fodder). Below the
  rails, UNCHAINED lists the tickets no chain holds, each row offering a
  `+ chain…` picker; it collapses once chains exist. Rails read the
  board's filtered task set (a step whose ticket is filtered out is
  hidden, chevrons reconnect what remains) and stay live through the
  board's SSE stream (`useBoardLive` invalidates `['board-workchains']`
  alongside the cards). The add-ticket picker rides the §7 Popover shell
  with search; the step-card summaries render off the chain read rather
  than the pills (which want a full Task). The skeleton grows a matching
  rail-shaped lens so the canvas shifts rail count, not widths. Wire
  types are the strict ones (`Effort`/`TaskStatus`); the pure rules
  (`buildPositions`, `chainProgress`, `chainedTaskIds`) live in
  `workchain-rules.ts` with the 9 unit tests, split from the
  svelte-query client (`workchain-client.ts`) so the node suite can run
  them. Verified: `bun run verify` green end to end (check + svelte-check
  0 errors + 1175 tests across 68 files), and the view driven in a real
  headless Chrome against a stubbed API — rails render with derived
  states (`1/4` progress, `HEAD` ring on the in-flight step), the lens
  round-trips through the URL against board/list/gantt, and clicking a
  card opens the ticket overlay (screenshots filed with the ticket).
  Live-data pass and the interactive writes (add step, reorder,
  pause/unpause) still need a dev stack.

- **Workchains: the handoff engine and the workchain-aware agent
  heartbeat.** A ticket landing in a done column now advances its chain:
  the new head's human assignees get an in-app `workchain_turn`
  notification ("It's your turn: <ref> - <title>", deep link
  `/boards/{board}/{task}`); an agent head hears nothing — its visibility is
  the heartbeat serving it. A ticket landing off-board (failed/cancelled)
  pauses the chain and notifies the chain's creator
  (`workchain_paused`, "Workchain paused: <chain> - <ref> <title>");
  unpausing is a human PATCH that re-derives the head, advancing nothing.
  The engine derives from the DB at write time — no in-memory chain state —
  and fires only on the normal status-write paths (`update_task` and the
  review sign-off in `complete_quality_review`), where human sign-off is
  the only trigger by construction (agents cannot land terminal columns).
  The agent heartbeat now enforces the chain's ordering: a step behind an
  earlier live step is excluded from `work_items` whatever its own column
  says, and a ready head carries `workchainReady: true` (absent on
  chain-free tickets — byte-identical feed shape for them). Verified:
  `bun run check` green; `cargo fmt --check` green; `cargo check --lib`
  green (the full clippy and test-target builds are SIGKILLed by the box's
  4 GiB cgroup — six attempts, every one an OOM kill with zero lint or
  compile findings surfaced; they must run in CI). The
  `derive_states`/`terminal_of` unit tests and the extended
  `workchains_live` router suite (4 more `#[ignore]`d tests: human-head
  turn notification, heartbeat ordering + ready flag + all-done,
  failed → pause + creator notification + unpause semantics, archived
  mid-chain head) need a dev Postgres + Redis to run.

### Added
- **Workchains: ordered ticket pipelines with human/agent handoffs (API +
  data model).** Two new tables — `task_workchains` (a named, position-
  ordered chain on a board) and `task_workchain_steps` (the chain's tickets
  in position order, unique per task across all chains) — land in the
  migrations array, with the CRUD API: `GET/POST /api/boards/{id}/workchains`
  (list with derived per-step state — done/head/waiting/archived, computed
  from the board's real status categories; create), `PATCH/DELETE
  /api/workchains/{id}` (rename, pause, reorder `positions: [{taskId,
  position}]` — all-or-nothing, a miss refuses the whole write; deleting a
  chain unlinks, it never deletes tickets), and `POST /api/workchains/{id}/
  steps` + `DELETE /api/workchains/{id}/steps/{taskId}` (add after an
  optional step, remove without touching the task). One chain per ticket is
  a v1 invariant backed by a unique index; cross-board adds answer 400,
  duplicates 409, unknown chains 403 like the boards family. Access mirrors
  board configuration: any member reads, owner/editor writes; every write
  bumps the board's SSE stream. Verified: `bun run check`; `cargo fmt`;
  `cargo check --lib`. Clippy and the full test pass are still owed — the
  box's 4 GiB cgroup, shared with sibling agent sessions, cannot fit the
  compile — as are the live proofs: `derive_states` unit tests (4,
  in-module) and the `workchains_live` router suite (6 `#[ignore]`d
  tests: create/list-with-derived-states, one-chain-per-task incl. the
  unique-index refusal, task-delete cascade, cross-board refusal,
  reorder + step-delete order honesty, chain-delete leaves tickets
  standing) need a dev Postgres + Redis.

- **Container-side tool RESULTS reach the run-detail modal — the talaria-events
  Hermes plugin.** Hermes' plugin API exposes `pre_tool_call`/`post_tool_call`
  with full arguments and results (the one datum no platform wire carried); a
  small Talaria plugin — embedded in the api, seeded into the fleet tree by
  render, mounted read-only at every agent's `~/.hermes/plugins/`, enabled in
  config.yaml — reports each call to the new agent-authed
  `POST /api/agents/tool-events`, which re-clamps, secret-scrubs, and lands
  `toolfull` frames on the live run's watch stream (live pane + retained
  transcripts, no Hermes fork). The plugin is best-effort by contract: a
  bounded queue and a short-timeout POST mean observability never blocks an
  agent's tool loop. Correlation is the agent's newest live work session
  (v1 approximation; exact persona-session pinning is the follow-up).
  Verified: cargo + ui gates; dev stack — render seeds and enables the
  plugin, and an agent-authed POST lands a `toolfull` frame on the watch
  replay with the args and result scrubbed (a planted `tak_…` arrived as
  `[redacted:key]`).

- **The desktop app auto-versions itself off main.** Every green build of a
  push that touches `desktop/` now mints the next minor version — highest
  suffix-free X.Y.Z across the `v*` and `desktop-v*` tags, minor+1 — and
  publishes it as `desktop-vX.Y.0`: a regular GitHub Release carrying all
  platform installers, `SHA256SUMS`, update signatures, and `latest.json`, so
  `/releases/latest` resolves to it and installed desktop apps auto-update
  (the feed flips only after every asset is uploaded). Until now trunk desktop
  builds were `0.0.0-sha…` artifacts that expired. Majors stay manual
  (dispatch `desktop-package` with `version=1.0.0, publish=true`); auto never
  crosses a major, and stable `vX.Y.Z` cuts raise the baseline. Verified:
  `bun run check`; the merge itself is the first live mint (resolve →
  desktop-vX.Y.0 → release + latest.json flipped) per RELEASING.md's new
  auto-minor section.
- **Full run observability — the run-detail modal replaces the small watch
  modal.** The old surface showed agent prose and tool names only; now every
  "watch the work" affordance opens a takeover modal with three panes. LIVE:
  the agent's stream with each tool call's argument preview (the persona's
  display-redacted primary argument — the whole terminal command, where the
  harness steering is legible) and the workbench's own MCP calls with full
  arguments and outcomes (`wtool` frames recorded at dispatch). TURNS: a
  retained per-turn transcript (prompt + stream) captured to a
  `run-transcript` artifact on the ticket at each turn's end — scrubbed of
  known credential shapes, bounded (16K prompt / 256K stream per turn), and
  retained per the new `observability.transcriptRetentionDays` admin setting
  (default 7 days, null = permanent; samples keep a 7-day cap). RESOURCES:
  the agent container's cpu/mem/pids sparklines over the run's window, from
  a new once-a-minute `agent-resource-sample` scheduler job over `docker
  stats` (new `agent_resource_samples` table; admin-only
  `GET /api/fleet/resources`). Known limit, documented: container-side tool
  RESULTS don't ride the persona wire — richer capture needs a Hermes-side
  change (follow-up). Verified: cargo + ui gates; dev stack — a work-session
  turn writes its transcript artifact (secret-shaped strings scrubbed), the
  modal's Turns pane parses prompt + tool-preview lines, wtool frames
  render live, the resources route answers (403 for non-admins), and the
  watch replay shows previews mid-stream.

- **Marketplace installs for package-shipped MCP servers (npm, pypi, and
  docker/oci images) — the GitHub-and-friends long tail.** The official
  registry's package-only entries — over a third of sampled search results —
  were invisible to the marketplace ("packages that need a local process
  can't be one-click added"); now they install like anything else. Each
  install becomes one hardened `docker run` child of the api: stdio packages
  (npm via `npx` in stock node, pypi via `uv tool run` in the bundled-python
  uv image, oci images directly) pipe JSON-RPC through a pump this process
  owns, and oci packages declaring an http transport run detached and relay
  like any remote. The container carries the Hermes chassis posture minus
  its cap_adds (no-new-privileges, cap-drop ALL, pids/memory/cpus ceilings,
  pinned DNS, default bridge network — no fleet network, no postgres, no
  docker socket), registry-declared `docker run` flags pass an allowlist
  (volumes/env/mounts/hosts/publish only) enforced at install AND spawn,
  images pull at install and pin by digest, and credentials use the same
  Input schema as hosted headers — sealed at rest, materialized into the
  child's environment only at spawn, never on any GET. Installs show a
  third-party-code warning with an explicit confirm (strongest wording for
  community tier); v1 runs one org-shared container per package (per-user
  auth is refused). Lazy spawn with respawn debounce, tools refresh and
  gateway dispatch through the pump, stop on disable/delete, and a boot +
  5-minute reconcile that sweeps strays. Verified live: installing
  `mcp-server-time` (pypi/uvx) pulls and digest-pins the runner image,
  discovers `get_current_time`/`convert_time`, answers a gateway
  `tools/call` with real data through the hardened container, re-answers
  `initialize` from the cached handshake, stops on disable, and self-heals
  after an api restart with no stray containers; browser-verified the pkg
  badge, the community warning, env fields, and the acknowledgement gate.

- **Workbench task concurrency is capped, with the sweep as the queue** — the
  fix for the 2026-09-17 outcrop freeze, where the engineering agent was
  offered 14 tickets at once, started 11 workbench jobs, and the accumulated
  per-job processes (vite, two Chrome clusters, tsservers, a Playwright
  install) OOM-killed its 4 GiB container 26 times until every work session
  blew its turn lease. Dispatch now offers a WORKBENCH agent at most 3 live
  work sessions (agents without a workbench are uncapped — their sessions are
  just model turns); a ticket past the cap is simply re-offered by the 60s
  sweep within a minute of a slot freeing — no queue table. `start_job`
  refuses a 4th live job naming the ones it has; approving a heavy plan at the
  cap 400s (reject always allowed). Workbench agents also render with
  `mem_limit: ${AGENT_WB_MEM_LIMIT:-8g}` (their sandbox runs builds, dev
  servers, and browsers; overridable per deployment like `AGENT_MEM_LIMIT`,
  lands on roll). Verified: cargo tests; dev stack — rendered compose carries
  the WB limit only on workbench agents; a workbench agent at 3 live sessions
  gets no 4th, and the ticket dispatches ~1 min after a slot frees;
  workbench-less agents dispatch past 3 unimpeded.

### Changed

- **Desktop packaging copy says what the app is.** The Flatpak/AppStream
  listing, `.desktop` comment, pacman `pkgdesc`, and installer descriptions
  call this the official Talaria desktop client, describe Talaria in the
  same voice as talariaworks.ai (one workspace, agents as teammates), and
  point homepage at https://talariaworks.ai. `stage.sh` now ships the
  metainfo into the FHS tree both packagers consume. Verified:
  `desktop-file-validate`; `appstreamcli validate --no-net`; `bun run check`.

- **Workbench coding harnesses are opencode, Pi, and Oh My Pi.** Claude Code
  and Codex are gone from the builtin registry and the seeded `dev` profile
  (a migration strips them from existing profiles and clears per-agent picks).
  All three authenticate through Talaria's gateway (`OPENAI_BASE_URL` /
  `OPENAI_API_KEY` / `LLM_WORKBENCH_API_KEY`) — Pi and Oh My Pi get a rendered
  `models.json` `talaria` provider. Hermes is the orchestrator, not a script:
  first turn is `jsonRun`, later turns are `continueJsonRun` (`-c`) against a
  per-job `--session-dir` (opencode continues by running again in the workdir).
  Print/json mode, no TUI; `--auto-approve` / `-a` so tool and trust prompts
  cannot hang. Invoke templates use `npx @latest` so CLIs auto-update; the
  workbench image preinstalls them and `talaria-harness-update` refreshes
  globals. Git in the sandbox uses `/usr/local/bin/git-credential-talaria`
  via `/etc/gitconfig` (`GIT_CONFIG_SYSTEM` set) — clone URLs carry no token.
  Skills teach driving, not forbidding the CLI. Dispatch forbids hand-coding
  and one-shotting. Verified: harness unit tests including `fill_harness_cmd`;
  `talaria_provider_models_json`; gitconfig pin; `bun run check`; live
  `--version` on opencode 1.18.31, pi 0.85.1, omp 18.2.4.

### Fixed

- **Workbench job branches now obey the repo's own branch rules.** `start_job`
  minted every job branch as `talaria/<ticket-ref>-<slug>`, but the platform's
  per-grant branch law (`workbench_repos.branch_prefix`, enforced on every
  sandbox push) can require a prefix — on outcrop-labs/talaria it is `agent/`,
  so every job branch on our own repo was unpushable from birth and
  `finish_job` could never see commits; PRs shipped only through the manual
  push-an-agent-branch-and-call-REST workaround (TALA-16 / PR #402), and job
  completion bookkeeping never fired. `start_job` now mints the job branch
  under the grant's configured prefix when one exists (the historical
  `talaria/…` shape otherwise), the job's rules text says the branch satisfies
  the repo's rules, and `finish_job` pre-checks the job's branch against the
  same law — a legacy job with an unpushable name gets an honest refusal
  pointing at abandon-and-restart instead of a misleading "no commits yet"
  after GitHub 404s the compare. Tools and docs updated to match. Verified:
  `cargo fmt`, clippy `-D warnings`, and the api test suite green, including
  the new self-referential test (a minted branch passes its own repo's
  `push_allowed`; the old `talaria/*` shape fails it).

- **The omapak Flatpak opened on "Could not connect to localhost: Connection
  refused".** Tauri treats the *absence* of the `custom-protocol` Cargo
  feature as `cfg(dev)` even for `--release`: `generate_context!` skips
  `frontendDist` and the window loads `tauri.conf.json`'s `devUrl`
  (`http://localhost:5290`). `tauri build` (GitHub Release installers) adds
  the feature; omapak's source build is a plain `cargo build --release` after
  `bun run build:vite`, so the published `app.talaria.desktop` was a Vite
  client with nothing listening. The crate now defines `custom-protocol`
  (`tauri/custom-protocol`); packagers that skip the CLI pass
  `--features custom-protocol`. It is not default: `generate_context!`
  panics when `frontendDist` is missing, and clippy/tests have no
  `desktop/dist`. Verified: `cargo metadata` lists the feature;
  `cargo tree -e features` without the flag does not enable
  `tauri/custom-protocol`; `bun run check`.

- **Marketplace servers that declare credentials lost their API keys on the
  way in.** The official registry declares remote headers as a `value`
  template ("Bearer {smithery_api_key}") with an optional `variables` map —
  and `classify()` parsed neither, so the install and per-user connect forms
  showed a bare header box, stored whatever was typed verbatim, and a pasted
  key left out the `Bearer ` prefix (upstream 401s); fixed publisher-set
  headers were dropped entirely, and the install POST's parser also stripped
  `isRequired`/`default`/`choices` from the stored declarations that drive
  the Settings → Connections form. The full `InputWithVariables` shape now
  flows registry → library wire → stored row → forms: a templated header
  renders one field per variable (secret-ness inherited, metadata from
  `variables`), the typed values are composed back into the final header, a
  literal `value` auto-applies with no prompt, and nothing is ever stored
  half-composed (`Bearer {key}` stays braces-intact until filled). Verified
  live: installing Smithery Notion from the marketplace prompts for
  `smithery_api_key → Authorization` and lands
  `Authorization: Bearer sk-…` on the server row; the per-user form renders
  the same field from the stored declaration; `bun run api:check` + `verify`.

- **OAuth connect failed on providers whose dynamic registration refuses
  hosted callback URLs (Vercel).** Vercel's DCR endpoint allowlists
  redirect URIs to localhost and a few known clients, so any deployed
  Talaria's callback gets `400 invalid_redirect_uri` — which `ensure_client`
  collapsed to the unhelpful "client registration failed (400)" while the
  server card's manual-app escape hatch stayed hidden behind its
  `dcr: true` flag. A refused registration now persists a `dcrRejected`
  marker on the OAuth config, the connect error carries the upstream's own
  reason plus the exact callback URL to register, and the card shows the
  manual-app setup (its dashboard app accepts custom callbacks; saving
  credentials clears the refusal and restores Connect). Discovery also
  falls back to the protected-resource document's `resource_documentation`,
  so Vercel's setup banner links its real MCP docs. Verified live against
  mcp.vercel.com: connect under a hosted origin answers the actionable
  sentence and sets the marker; saving a manual client clears it and
  re-arms Connect; `oauth_meta` matrix + sentence pinned in tests.

- **Stripe's MCP server was un-connectable: OAuth discovery never found its
  authorization-server metadata.** Stripe's issuer URL carries a path
  (`https://access.stripe.com/mcp`) and serves metadata at the RFC 8414
  location — the well-known segment before the path — which was the one
  shape `discover_oauth` didn't try, so Stripe installed as a plain
  header-auth server with no Connect flow at all. The candidate set (now
  extracted and test-pinned) tries every well-known shape; probed the other
  marketplace majors while in there — Notion, Linear, Airtable, and PayPal
  register hosted callbacks out of the box, Figma and Asana refuse DCR and
  land in the manual-app flow above, and GitHub keeps its documented
  cross-domain pin. Verified live: registering `mcp.stripe.com` now
  discovers OAuth (`dcr: true`) and a connect start 302s into Stripe's
  authorize endpoint.

- **Hobby apps on `*.vercel.app` wore the gold "official" badge in
  marketplace search.** An `app.vercel.<project>` namespace reverses to
  `<project>.vercel.app`, and a remote on that same host promoted the entry
  to first-party — the platform's badge on a tenant. Shared-hosting
  suffixes (vercel.app, netlify.app, pages.dev, workers.dev, github.io,
  gitlab.io, fly.dev, deno.dev) now demote to community. Verified live:
  `agent-svg-registry` search answers `community`; registry-shaped fixtures
  pinned in the library tests.

- **The api package image failed to compile on `main`.** `hermes_skills.rs`
  `include_str!`s `scripts/hermes-skill-authority.json` from repo root;
  `package.Dockerfile` had flattened `api/` onto `/repo`, so the path was
  `/scripts/...` and missing. The build now keeps the repo layout
  (`/repo/api` + `/repo/scripts/...`). Verified: the previous `main` package
  job failed on that exact error; this file is the fix.

- **Every page 404'd in production while `/api` kept working.** The server
  build now splits into `dist/server/assets/*.js` chunks — one directory
  deeper than the `dist/server/server.js` the SPA-shell lookup was anchored
  to — so `readFile('../client/index.html', import.meta.url)` threw, the
  handler cached `shell = null`, and all four deployed instances (dogfood ×3
  + bbills) served plain `404 Not Found` for `/`, `/home`, `/login`, … Dev
  mode never runs the shell path (vite serves `index.html` itself) and no
  gate executed the built bundle, so CI was green on it. The shell now
  resolves from `process.cwd()` (`ui/` in dev and under server-entry alike —
  the rule `app-build/paths.ts` states). Verified: `bun scripts/check-prod-shell.ts`
  red on the pre-fix build, green after; `bun run check`; ui test + typecheck.

### Added

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

- **CI smokes the built bundle.** The `ui` CI job now runs
  `bun run build` + `bun scripts/check-prod-shell.ts`: it imports the real
  `dist/server/server.js` and asserts GET `/` (and a deep client route)
  serve the SPA shell and that `/api` paths never leak it. This is the gate
  that would have caught the 404 outage at PR time instead of on the fleet.

- **Hermes bundled skills stay classified, and every pack we prune occupies
  the name agents reach for.** Hermes ships Notion, Obsidian, Airtable, gh,
  gws, Himalaya, Box, xlsx, llm-wiki, raw coding-harness CLIs, and more —
  and adds packs on image updates. A six-path prune array in docker.rs
  silently let new conflicts in, and only `github` had a Talaria signpost,
  so a search for "notion" found a hole and the model improvised. Source of
  truth is `scripts/hermes-skill-authority.json`: every snapshot path is
  replaced, keepExact, or keepPrefix; unclassified fails `bun run check`.
  Replaced packs are `rm -rf`'d on every container roll (`hermes_skills::
  prune_paths`); a short SKILL.md at `scripts/skills/<signpost>/` occupies
  the Hermes `name:` (email, obsidian, notion, airtable, google-workspace,
  box, xlsx, llm-wiki, claude-code, codex, opencode, xurl,
  teams-meeting-pipeline — github already existed). Fitness: `hermes:authority`
  (six fixtures — Notion/Obsidian/Excel/Box/wiki/Airtable asks must hit
  Talaria tools). keepPrefixes is apple/ only — every other family is
  keepExact so a new creative/ or web/ pack cannot sneak in. The chassis boot
  smoke `find`s SKILL.md in the live image and fails on unclassified packs.
  `update_document` takes `rows`/`html` and refuses markdown on a sheet or
  page (that would smash the grid). Soul-header bullets generate from
  `TALARIA_TOOLS` so a new tool cannot miss the contract. `hermes:authority`
  fails a reply that called the right tool then claimed "saved to Notion".
  Verified: `bun run check`; `cargo fmt`; `cargo clippy --lib -- -D warnings`;
  `cargo test --lib` hermes_skills, hermes_authority, talaria_tools, sandbox,
  org, registry.

- **Agents can now author spreadsheets and web pages, reply in threads, and
  react — and the fitness suite measures whether a model uses those tools.**
  The built-in toolkit was missing three teammate-shaped verbs the HTTP API
  already allowed: `create_sheet` (Files spreadsheet, JSON `string[][]` with
  row 0 the header), `create_page` (HTML microsite), and `react_to_message`
  (the dual-auth reaction route, under the agent's own identity).
  `post_to_channel` takes `threadId` so a reply stays in the thread;
  `read_channel` takes the same id to read one thread. The talaria-toolkit
  skill teaches the reflexes (grid ≠ markdown table; a ✅ is not a new post).
  Fitness: catalog 58 → 61 with sandbox backends; new `hermes:comms` harness
  (six fixtures: read before post, react don't chatter, replies stay in
  thread, the room not a DM, ids from listings, don't spam DMs) bound to the
  workspace-agent fleet slot; `hermes:documents` gains a spreadsheet-vs-
  markdown-table fixture. Guardrails unchanged (no assign, no complete).
  Verified: `bun run check`; `cargo fmt`; `cargo clippy --lib -- -D warnings`;
  `cargo test --lib` on `hermes_comms`, `hermes_documents`, `talaria_tools`,
  `toolbox::sandbox`, `registry::tests`, and `score::tests`.

- **Desktop updates itself from a GitHub Release.** Settings → Profile (and
  the launcher) Check for updates reads `/releases/latest/download/latest.json`,
  verifies a minisign signature, replaces the install, and relaunches. Stable
  tags attach `latest.json` plus `.sig` files for the AppImage, the universal
  `.app.tar.gz`, and the NSIS installer; RCs do not (GitHub's `/releases/latest`
  is the stable pointer). Verified: `write-latest-json.py --self-test`; cargo
  test + clippy on the new `check_for_update` / `install_update` commands.
- **Teams are first-class.** They are no longer a boards-only grouping:
  Manage → Teams (`/teams`) is a LibraryPane of org teams (people + agents),
  with admin view grants, permission overrides, and MCP tool rules on the
  team itself. Adding a team to a channel, plan, research run, KB doc, or
  artifact expands at auth time — roster changes apply without rewriting
  grants. Boards still use `boards.team_id` as ownership. Verified: `bun run
  check`; `cargo clippy -D warnings`; `kb::perms` tests (10); minted an
  admin session on the `teams` worktree stack (`:5305`) and created
  Engineering via `POST /api/teams`, granted `/mcp` + `kb.official` via
  `PUT /api/teams/{id}/access`, then opened Manage → Teams (nav, picker,
  people/agents/access chips) and MCP → Manage access (Teams section).
- **Agent refines announce themselves: the silent document swap is gone.**
  When an agent edits an open document through its toolkit
  (`edit_kb_doc`, `update_document`), the surface now says so — an
  "Agent refine" notice on the KB doc and artifact views (read and edit
  panes alike) naming who wrote the revision, when, and a change summary
  (±lines), with a Show-changes diff against what the viewer held when the
  refine landed, a Load-into-editor review path, and a per-viewer dismiss
  that persists across remounts (localStorage, keyed by viewer identity).
  The updated body appears without a manual reload: read panes follow the
  server on announce; the editor's buffer is never yanked — the refine
  stages behind the notice and lands only when the viewer loads it. A
  viewer's own saves are never announced, detection polls `/api/history`
  every 4s, and the notice announces the newest unseen revision only —
  one refine at a time, the same way a reviewer reads. Decisions live in
  `ui/src/lib/agent-refine` (unit-tested, 17 cases), the DOM in
  `AgentRefineNotice.svelte`. The Muse accept stopped being a vanishing
  act too: it folds with a ±line receipt, and the Draft/Refine button
  carries a pulsing in-progress state while streaming. Muse failures keep
  their error line. Verified in a real browser against an API stub: the
  notice announces, the diff opens with the refined text, load stages and
  saves, read panes refresh without a reload, the receipt shows the
  accept's change, a failed Muse run shows the error, and the artifact
  surface announces the same way (screenshots in the ticket).
- **New-agent onboarding got the same treatment: the Muse refine of an
  agent design is no longer a silent field swap.** On the review step of
  the new-agent modal, a refine now shows an honest in-progress state
  ("Refining the design: identity, soul, and starter skills" with a pulse)
  for the seconds the whole-agent contract takes, and on landing it folds
  with a "Refine applied" receipt — which fields the design touched and
  the ±lines on the soul — that holds until the next refine. The describe
  step's first design keeps its generating block. Decisions live in
  `ui/src/lib/agent-onboard-refine` (unit-tested, 10 cases); verified in a
  real browser against an API stub: design in-progress state, draft lands
  in the review fields, refine in-flight state, receipt math on a trimmed
  soul, failed refine leaves the draft untouched, error line shows.
- **`talaria deploy` honors `COMPOSE_FILE` — the registry-image flow works
  through the wrappers, no build on the host.** Export the layered file list
  (`docker/compose.yml:docker/compose.registry.yml[:your-vm override]`) and
  every deploy leaf drops its explicit `-f` (docker's own precedence puts -f
  ABOVE the env, so honoring the env means stepping aside); registry mode
  pulls `talaria searxng-config` fail-fast and `up` runs WITHOUT `--build` —
  the override swaps the image but cannot remove the base's `build:` key, so
  a build would tag the checkout as the registry ref. `talaria service
  install` captures the env into the systemd unit, and the env-drift warning
  scans every file in the list (so `TALARIA_CHANNEL` and per-override knobs
  join the checked set). Verified: `bun talaria deploy up/update/down` argv
  under COMPOSE_FILE asserted in the CLI suite (pull-before-up ordering, no
  --build, die-on-pull-failure), unit text with and without the env, drift
  across a layered override, and the unset path byte-identical to before.
- **Enable compiles and loads the app first.** Manage → Apps runs a rebuild +
  module probe before enable (UI surfaces, `server.ts` `fetch`, `mcp.ts` `tools`);
  a failed compile or a module that will not load is refused with that error.
  An already-enabled app whose `current.json` is `failed` is disabled by the
  boot reconciler (api + UI), not by GET `/api/admin/apps`. Verified:
  `enable_block_reason` names a failed compile and lets `ready` through;
  `moduleHasFetch` / `moduleHasMcpTools` / `moduleHasSurfaces` accept the SDK
  shapes and reject empty defaults; `clientArtifactOk` requires an ESM export.

- **`talaria app new <slug>` scaffolds a TypeScript app** (work surface, document-store
  server, MCP starter) into `apps/<slug>`. Authors stay on `@talaria/sdk`; never Rust.
  The fleet skill `talaria-apps` is the playbook for building one. Verified: slug
  guard refuses before write; existing dest dies; files land under `apps/<slug>`
  (or `TALARIA_APPS_DIR`); skeleton embeds the slug in the client only.

- **Apps compile on the instance, not into the host.** Installing an app no
  longer requires rebuilding Talaria. The host emits stable `/runtime/rt-*.js`
  entries; each app is a standalone Vite build against those, with its own
  Postgres (a compose project, spawned on install, destroyed on uninstall).
  The DB password is sealed in `app_settings` and written to a 0600 `db.env`,
  never `docker run -e` and never HMAC of `TALARIA_SECRET_KEY`. `talaria backup`
  dumps each app DB into `app-data.tar.gz`. A throw in an app's UI, server,
  or MCP stays in that pane and shows the crash — the rest of the cockpit keeps
  running. Authors write TypeScript against `@talaria/sdk`; they never write
  Rust. Core ships no example app — scaffold with `bun talaria app new`.
  Verified: `bun run verify` (svelte-check 0 errors, mcp `tsc --noEmit`,
  1139 ui tests); `cargo fmt` + clippy `-D warnings` + 2112 api tests from
  `api/` (the crate's rust-toolchain). Specifier derivation includes
  `svelte/internal/client` and excludes the compiler; `sourceKey` is stable
  then changes with a source edit; `isolateApp` swallows a throw into
  `{ ok: false }`; `composeYaml` has no password; a compose down/up kept a
  row; restore of `all` extracts `app-data.tar.gz`.


- **Talaria Desktop — a Tauri v2 multitenant shell around Talaria instances
  (`desktop/`, [`docs/DESKTOP.md`](./docs/DESKTOP.md)).** One window, two
  views: the launcher's welcome screen (the only local frontend — wing mark
  on the signature dither, Mercury typography, the instance list, add), and
  the active instance's own web UI as the entire window — the interior is
  always the real UI, never a second codebase. Switching lives INSIDE the
  product: the desktop switcher (`ui/src/components/app/DesktopSwitcher.svelte`)
  wears the current instance's identity beside the logo in the nav rail and
  in the login screen's corner, rendering only inside the shell
  (`inDesktopShell()`, feature-detected — a browser gets nothing);
  Ctrl/Cmd+Shift+H opens the launcher even on instances whose deployed UI
  predates the switcher. Adding an instance validates it against the
  instance beacon (`/api/well-known/talaria-instance`) and dedupes by
  instance uuid; each instance webview gets its own data directory, so
  sessions on the same host at different ports never collide, and hidden
  webviews stay loaded (SSE survives a switch). Instance origins are granted
  exactly three commands at runtime (list / activate / show-welcome) via
  Tauri's dynamic-ACL — remote content can switch instances and nothing
  else. `decorations: false` (no GTK titlebar where the WM shows none).
  Gates: `bun run desktop:check` runs in a devbox and CI's new `desktop`
  job; the GUI runs on the host — the devbox image gained the webkit2gtk
  build deps. Verified live: devbox instance + the real hosted
  `talaria.outcroplabs.com` added through the dialog, signed in, and
  switched between via the in-UI switcher; the active instance fills the
  window at any size (an earlier persistent-sidebar design stacked the two
  webviews vertically inside WebKitGTK and drowned the app — exactly-one-
  visible-webview is the fix; manual bounds remain as belt-and-suspenders).

- **Talaria Desktop ships installers for all three desktops, built and
  attached by CI (`.github/workflows/desktop-package.yml`,
  [`docs/DESKTOP.md`](./docs/DESKTOP.md)).** Tauri's bundlers cover linux
  deb/rpm/AppImage, macOS as one universal build, and windows nsis (plus an
  `.msi` when the version has no pre-release part — Windows product versions
  have a numeric fourth field, so a `v0.2.0-rc.1` release ships the `.exe`
  rather than an `.msi` claiming to be `0.2.0`); the two targets Tauri has none
  of — Arch's pacman and flatpak — are built from one staged FHS tree in
  `desktop/packaging/` (`stage.sh`, then `pack-pacman.sh`, or
  `flatpak-builder` against the GNOME 50 runtime). The bundle targets moved
  out of the previously-empty `targets` list into per-platform config files
  (`tauri.linux.conf.json`, `tauri.windows.conf.json`,
  `tauri.macos.conf.json`), so `tauri build` produces that platform's
  installers instead of a bare binary — and the released version is merged in
  at build time (`--config`), because the tag is the version authority
  (RELEASING.md) and nothing in the repo carries a released version. On a
  `vX.Y.Z[-rc.N]` tag, `release.yml` calls the workflow once the image is
  pushed and every installer lands on that GitHub Release with a
  `SHA256SUMS`; a push to main touching `desktop/` leaves the same set as
  workflow artifacts. Nothing is signed or notarized — no Apple Developer
  certificate and no Windows signing key exist here — so macOS wants a
  right-click → Open the first time and Windows warns through SmartScreen.
  Verified: `bun run desktop:check` green (fmt, clippy, 14 tests,
  svelte-check); a local `tauri build` produced the release binary and a
  `.deb` stamped with the injected version (`Version: 0.0.0-test`,
  `Depends: libwebkit2gtk-4.1-0, libgtk-3-0`); `packaging/linux/*.sh` turned
  that binary into a `.pkg.tar.zst` that `pacman -Qip` reads back
  (`talaria-desktop-bin 0.2.0_rc.1-1`, deps `webkit2gtk-4.1`/`gtk3`, x86_64).
  The full matrix then ran in CI
  ([run](https://github.com/outcrop-labs/talaria/actions/runs/35064711777)):
  six jobs green, producing
  `Talaria_<v>_amd64.{deb,AppImage}`, `Talaria-<v>-1.x86_64.rpm`,
  `Talaria-<v>-x86_64.pkg.tar.zst`, `Talaria-<v>-x86_64.flatpak`,
  `Talaria_<v>_universal.dmg` + `.app.zip`, and the NSIS `.exe` — whose
  control metadata, `pacman -Qip` output, DMG/PE headers and the AppImage's
  ELF were all read back afterwards. Two of those runs earned their keep:
  flatpak-builder needs `eu-strip` (elfutils), and the msi bundler refuses a
  pre-release version, hence the conditional `.msi`. A third did: the
  CI-built AppImage aborted on Arch (`Could not create surfaceless EGL
  display: EGL_BAD_ALLOC`) — bisected to linuxdeploy's bundled
  `libwayland-client.so.0`, excluded by a second repack pass
  (`packaging/linux/repack-appimage.sh`) — and the repacked artifact was then
  run on that same host: window up, the registered instance loaded.
  Each platform's own job then proves the artifact comes up, not merely that
  it built
  ([run](https://github.com/outcrop-labs/talaria/actions/runs/35098785546)):
  macOS verifies the dmg (`disk image (Apple_HFS : 4): verified`) and launches
  the .app (still running 20 s later), windows installs the installer it just
  produced and launches what it installed
  (`C:\Users\…\AppData\Local\Talaria\talaria-desktop.exe`, still running), and
  the deb, rpm, pacman and flatpak packages were each installed in a clean
  container of their own distribution (Ubuntu, Fedora, Arch). What stays
  unverified is what no runner can be: Gatekeeper and SmartScreen as a user
  meets them — an artifact built on a runner was never quarantined — and
  session isolation on macOS/Windows, which `data_directory` does not provide.

### Changed

- **The release pipeline ran for the first time, and the nightly feed came back
  to life.** No `v*` tag had ever existed here, and every scheduled nightly for
  at least the eleven runs before 2026-09-16 had died as a `startup_failure` —
  a run with no jobs and no logs, which is why the channel could be dead for
  eleven days without anyone noticing. Cutting `v0.1.0-rc.1` surfaced three
  defects that only a real run could: **(1)** a called workflow's jobs may
  request no more than the CALLING JOB grants, so `release.yml`'s nested
  `api-package` call was refused for asking `packages: write` under a
  `contents: read` caller — the grants now sit on the calls, not raised
  workflow-wide; **(2)** the installers' `attach` job ran `gh release upload`
  without ever checking the repository out, and `gh` resolves the repository
  from a local clone, so the first run stopped at the very last step
  (`failed to run git: not a git repository`) with every artifact correct and
  in hand; **(3)** `testing`, the ref a nightly builds, was three weeks stale
  (a 2026-08-26 tree with no `api/` and no `desktop/`), so the first nightly
  that got as far as building found directories that did not exist. All three
  are fixed, the channel was advanced, and `v0.1.0-rc.1` published: GHCR
  `0.1.0-rc.1` + `rc` for both the app image and the api package, a GitHub
  prerelease carrying every installer plus a `SHA256SUMS`
  ([release](https://github.com/outcrop-labs/talaria/actions/runs/35111577866)),
  and the first working nightly in the channel's recorded history —
  `nightly` + `nightly-YYYYMMDD`
  ([nightly](https://github.com/outcrop-labs/talaria/actions/runs/35111665940)).
  `RELEASING.md` now names the two ways a nightly stops for good: the 60-day
  schedule rule, and a startup failure whose reason lives only in the run page's
  banner. A second desktop-installer workflow, merged from a parallel session
  after that release (triggering on the same `v*` tags, uploading to the same
  release with `--clobber`, and asking Tauri for a `flatpak` bundle type it does
  not have), was removed in favour of the one `release.yml` actually calls: one
  publisher per tag.

- **CI checks the surface that moved, not the whole tree.** A UI change no
  longer re-runs the Rust api's `fmt`/`clippy`/`test`, a desktop change does not
  re-run the UI suite, and a docs-only push runs the invariants job and nothing
  else — decided per job, inside the run, by a new `changes` job in
  `.github/workflows/ci.yml` (`.github/workflows/migrations.yml` gets the
  equivalent trigger filters, its only inputs being the migration array in
  `ui/src/server/db/pg.ts` and the snapshot beside it). The invariants job stays
  unconditional on purpose: it is seconds long, needs no install, and a
  "docs-only" change is exactly what breaks a doc link. Trigger-level `paths:`
  are still out — a workflow-level filter is as coarse as the file, and a
  filtered-out required check would report "pending" forever — and every unknown
  resolves to the safe side: a first push, an unresolvable diff range, or a
  `release.yml` call runs EVERY surface, because a publish is never a partial
  gate. Verified: the decision logic was extracted verbatim and run against a
  fixture repository for every path (ui-only, api-only, desktop-only, docs-only,
  workflow-file, zero-before push, called-from-release, unresolvable range), each
  producing the intended set; and `main`'s protection was read back to confirm it
  requires a review and **no status checks**, so a skipped job cannot wedge a
  merge the way a filtered-out required check would. The skip path itself becomes
  observable on the first push to main that touches no code.

- **The coding harness is the mandated path for code work — named, keyed,
  unblocked, and skilled.** Four pieces, one contract: the agent that was
  handed a harness drives it instead of hand-coding. The dispatch brief now
  NAMES the agent's selected harness (the platform knows workbench_harness —
  no "whichever is configured" hedging) and points at `doctor` for its guide;
  hand-editing files is reserved for trivial one-line fixes, and a harness
  the agent cannot drive is a report_gap, never a reason to silently
  hand-code. Claude Code's auth finds the org's model access wherever it
  lives, through a three-step lookup that never needs an "anthropic" endpoint
  to be configured (the platform's endpoint rows are OpenAI-shaped by
  construction): a REFERENCE TABLE of providers with known fixed
  Anthropic-protocol surfaces (anthropic native; OpenRouter's first-party
  /api/anthropic; DeepSeek's /anthropic) answers by slug with zero network;
  otherwise a one-time probe of {base}/v1/messages — the row's own base URL,
  or the origin of the provider's native base — verifies the surface and
  CACHES the verdict on the endpoint row (llm_endpoints.anthropic_base), so
  the network half runs at most once per endpoint for the life of the
  install; the hit becomes ANTHROPIC_BASE_URL with the same key riding
  ANTHROPIC_AUTH_TOKEN — no OAuth login — and the no-key-anywhere case now
  WARNS at render instead of arming a harness that fails silently
  (the silence that hid harness non-use across the fleet: zero Claude Code
  sessions ever, zero workbench jobs ever, Hermes hand-coding everything).
  Every armed harness also runs unattended-clean and skilled: onboarding
  cleared and permissions bypassed via read-only policy files mounted at its
  CLAUDE_CONFIG_DIR paths, the fleet skills HOST DIRECTORY bind-mounted
  directly into Claude Code's and Codex's skill directories (a symlink to
  the container-only /opt/skills dangles on the host, and the docker daemon
  answers a dangling bind source with mkdir "file exists" — the first roll
  after the initial merge 500'd every agent up; the direct mount is the
  fix), and AGENTS.md/CLAUDE.md pointers in the workspace telling every
  harness where the skills live.


- **The last plain-JS sources are TypeScript now: `server-entry`, the svelte
  config, and the service worker.** `server-entry.ts` is the one that
  matters — it was outside the tsconfig `include`, so the production server
  (env loading, static serving, the SSE pump, boot migrations, the Rust-api
  supervisor) had never been typechecked; it now carries full types (the
  dist bundle import is typed from `src/server/app.ts`, the one module
  whose exports survive into it). `svelte.config.ts` is supported natively
  by vite-plugin-svelte 6. The service worker moved from `public/sw.js`
  (served verbatim) to `src/sw.ts` as a second client-build entry emitted
  unhashed at `/sw.js` — the registration URL in browser-notify.ts is
  unchanged — with a small dev middleware serving the same URL so
  dev-mode registration keeps working. `bun server-entry.ts` replaces
  `bun server-entry.js` in the start script, the container entrypoint, and
  the image's runtime COPY.
  The stdlib-only `.mjs` under `scripts/` and `docker/` stay as they are:
  they must run under any node with no install, which `.ts` would break.
  Verified: `bun run verify` green (typecheck now covering the entry), a
  built-from-scratch `dist/` emits `sw.js` at the client root, and a boot
  of the built server against a scratch database listens, answers
  `/api/healthz`, and serves `/sw.js` with a JavaScript content type.

### Fixed

- **The plan document builds itself again: server-side sync replaces the
  client's witnessed-landing guess (TALA-33).** The plan's living document was
  rewritten only when a client POSTed `/api/plans/:id/doc` — and the client
  fired that only when THIS tab watched a turn's in-flight → complete flip.
  Three ordinary gaps silenced it: a reply longer than the chat view's ~4
  minute resume poller, a turn landing behind a queued message (the landing
  check read only the last row's role), and a stream-death retry whose
  completion the tab never saw. The pane then sat stale until someone pressed
  "Sync from chat" by hand. Now the server calls the same `sync_plan_doc`
  rewrite (whole-document contract and data-loss guard intact, `tier` routed
  and metered exactly like the manual route) as a detached task when a plan
  turn persists complete; a per-plan in-flight guard skips overlaps and a
  5-second recency window skips a double-burn behind a manual sync the client
  already fired. The doc pane listens on the conversation event firehose, so
  it refetches and renders the new version even for landings this tab never
  witnessed or that other members' tabs triggered. The chat view's landing
  arm is conversation-scoped (a turn landing behind a queued row now fires
  `onTurnComplete`) and the resume poller lost its tick cap — its lifetime is
  the turn's, not four minutes. "Sync from chat" stays as the explicit
  fallback, and a failed auto-sync logs loudly rather than failing the turn.
  Verified: `cargo test --lib chat_persist` (auto-sync policy suite), clippy
  `-D warnings`, `turn-landing.test.ts` (6 tests) in vitest.
- **Boards crash under WebKit with "Can't find variable: requestIdleCallback".**
  `BoardLayout.svelte` feature-detected the global with
  `requestIdleCallback ?? fallback` — but reading an absent global by name
  throws ReferenceError before `??` ever runs, so WebKit (Safari, and
  WebKitGTK — the desktop shell's engine) died opening any board. The guard
  is now `typeof requestIdleCallback !== 'function'`, with the fallback and
  its cleanup correctly paired. Found in the desktop shell on 2026-09-15;
  it was equally broken for browser Safari.
- **ui's typecheck no longer sweeps gitignored subrepo apps.** Any machine
  with `apps/leadworks`/`apps/waypoint` checked out sprayed ~100 phantom
  "Cannot find module" errors into every svelte-check (subrepo imports
  resolve against their own absent node_modules), which is why CI disagreed
  with every local run. The subrepos are now excluded in `ui/tsconfig.json`,
  and a new `bun run check` invariant (`subrepo-app-inside-the-ui-tsconfig`)
  fails in seconds when a future subrepo appears unexcluded.
- **The launcher's add-instance dialog rendered behind the content that
  opened it** — the welcome content sits at `z-10` and the dialog had no
  z-index. Both overlays are `z-50` now.
- **The bundled Hermes github skill is pruned from fleet containers, and a
  Talaria-authored `github` skill stands in its place.** The image ships a
  gh-CLI-first github pack whose preflight (`gh auth status`) and auth
  workflows pitch exactly what Talaria forbids — there is no `gh` in the
  containers, and the credential is injected at git time, never visible.
  Observed live on outcrop (2026-09-15): an agent opened the skill, noted it
  "assumes gh CLI", and recovered only because its memory notes said
  otherwise. The pack joins `CONFLICTING_SKILL_PACKS` (pruned on every
  container roll, the same mechanism as the five note-tool packs), while
  `scripts/skills/github` seeds the shared root with a signpost skill at the
  exact name agents reach for — plain git over https, the workbench opens
  the PR, no auth to set up — routing to the talaria-toolkit and
  workbench-driving sections that carry the methodology. Verified in an
  isolated worktree render: the skill seeds into the fleet shared root and
  lists as a platform (admin-locked) skill beside the toolkit; the prune
  path matches the live dogfood container's pack layout.

- **`execute_code` works again on fleet agents: rendered configs now carry
  `approvals: { unattended_mode: approve, cron_mode: approve }`.** Talaria
  drives every agent through the Hermes api — an "unattended platform" in
  Hermes' approval model — and cron jobs run the same way, so the fail-closed
  defaults denied `execute_code` outright (the live error: "This session runs
  on an unattended platform (api_server) with no user present to approve it")
  and stalled every dangerous-command prompt until timeout; agents limped
  back to terminal one-shots. The container is the sandbox and tirith is the
  guard, so both modes flip to approve, preserving any other approvals keys
  an agent def carries. Verified: a unit test pins the override (authored
  keys preserved, both modes flipped), and a live render in an isolated
  worktree emits the approvals block into the agent's config.yaml.

- **Desktop switcher, titlebar, unnamed instances, and in-app files.** Four
  desktop-app bugs in one pass. The instance switcher in the left nav opened
  `align="right"`, so the menu painted off the left edge of the window —
  dropdowns now clamp to the viewport and the switcher always opens to the
  right of its trigger. macOS and Windows had no working titlebar
  (`decorations: false` and no custom chrome): Settings → Profile (and the
  launcher) now pick Themed / OS / None, Themed by default on every OS, with
  drag + min/max/close following traffic-light side. Switching dropped
  instances that had no company name because the row matched on a blank
  label — the switcher now matches beacon uuid then origin, and labels fall
  back to the host. Chat (and board) file chips opened `target="_blank"` on
  `/api/uploads/…`, which in the desktop webview left the app with no Save;
  every such file now opens an in-app modal (preview when we can, "cannot
  be previewed" when we cannot, Download either way, like Drive). Verified:
  `dropdownHorizStyle` and `findCurrentInstance` / `instanceDisplayLabel`
  unit tests; `sanitizeFilename`; cargo tests for settings default/roundtrip.

### Added

- **Watch the work is a terminal now: the agent's own words and tool calls,
  streaming live from the run — on the board card, not buried in the ticket.**
  A work-session turn tees every parsed stream event (prose as it lands,
  tool calls as they start, reasoning, failures) to `run-watch:<id>` with a
  per-turn replay tail; `GET /api/runs/{id}/watch` (run-ACL-gated like the
  run's event stream) answers an SSE of bounded replay then live frames.
  The watch modal renders it as a pinned terminal — tool markers on their
  own lines, flowing prose beneath, previous turn's reply tucked behind a
  disclosure. Board cards with live sessions now wear the dither field
  themselves (a working cell before the columns, one board-wide sessions
  read), with the watch affordance ON THE CARD; the ticket strip keeps its
  live phase. Turn counters show `turn N` — no invented "/12" denominator —
  and the modal never skeleton-locks: the terminal opens with whatever the
  turn has produced so far, or says it is waiting for the next output. One
  new door verb, `getStream` (the GET twin of `postStream`), carries the
  client side through the one-HTTP-door invariant (count 7).

- **A ticket an agent is actively working wears the work: a dithered
  live-work strip with a Watch-the-work modal.** While a work session is
  live on a ticket, the detail view carries the same dither field the
  generating blocks wear — "work is happening here" — with the agent's name,
  the turn count, and the live phase sentence; a CTA opens a modal that
  streams the session's own run events (`/api/runs/{id}/events`) for
  instant phase transitions, shows the tail of the agent's last reply (the
  session's checkpointed answer, not a re-read of the model), and refreshes
  the ticket's activity and comments as work lands. Backed by a new
  `GET /api/tasks/{id}/work-session` (board-gated like the ticket read)
  returning the live session row or null. A window into the work, not
  control of it — nothing in the modal touches the session.


- **Per-project env stores: an agent's build gets its `.env`, sealed at rest
  and scoped by the repo grant.** Each granted repository can carry a set of
  environment variables in the workbench admin (beside the branch rules):
  KEY chips with write-only values — a value never crosses the API in either
  direction after it is saved; it is sealed with the same envelope the
  workspace secret vault uses and leaves the database exactly once, when the
  fleet render materializes `/opt/workbench-env/owner/repo.env` into the
  containers of the agents granted that repo (read-only mount, shell-safe
  single-quoting, absent entirely when the store is empty). The work-session
  brief names the file's path when one exists, so the agent sources it the
  way a developer would. The visibility class is the inverse of the vault by
  design and the panel says so: these values ARE readable by the agent's
  build — exactly what a project `.env` is for; org credentials stay in
  workspace secrets, which are never revealed to anything. Access control
  needs no new grants: the existing repo grant IS the env grant. Verified
  live: a variable set through the API arrives verbatim in the granted
  agent's container file and in no other agent's, and the stored row is
  ciphertext.


- **Per-agent branch rules, administered against the repo's real branches,
  enforced by the platform before a byte leaves the container.** Every repo
  grant now carries its own branch law: the base branch a human merges
  (picked from the repo's live branch list, defaulting to its default
  branch), whether the agent may push the base at all (`branches_only` is
  the zero-config default posture — work lives on a branch; `free` is the
  explicit opt-in), and an optional required prefix for the agent's
  branches. The gates pass down the whole chain: the admin panel edits rules
  beside the grant chips with real branch pickers; the work-session brief's
  hygiene step is personalized from them (each granted repo's base and
  prefix, with the standing default when unconfigured); and a rendered
  `pre-push` hook — installed system-wide via `core.hooksPath`, chaining to
  a repo's own hook when one exists — asks `/api/secrets/git-push-check`
  over the agent's key with the refs git is about to push, so the platform
  itself declines a base-branch or off-prefix push even on repos GitHub's
  free plan cannot protect. Grant toggles no longer wipe rules (set
  semantics, not delete-and-reinsert). Verified: the decision table is
  pinned by unit tests, the generated hook is proven valid shell by `sh -n`
  in the test suite, and live from an agent container a base push is
  declined with the rule's own sentence while a conforming branch push
  passes.


- **An agent's code reaches main only through a human's merge.** The work
  session's dispatch brief now carries the repo-hygiene law as a numbered
  step — push to a branch named for the ticket, open a PR, put the link in
  the outcome; the toolkit skill states the same rule for every other
  surface — and it is enforced where behavior cannot be: `main` on the
  granted repos now requires a pull request (branch protection, zero
  approvals needed, administrators exempt), so an installation-token push to
  main is declined by GitHub while human direct pushes are unchanged.
  Verified live from inside an agent container with the injected credential:
  a branch push succeeds (`hygiene-probe` created and cleaned up), a push to
  main is refused (`protected branch hook declined`). Two private repos
  (talaria-website, skal-website) cannot carry protection on the org's free
  plan — the prompt and skill rules stand there, and upgrading the plan or a
  push-webhook revert are the options if they need the hard wall too.


- **Enter in the chat composer reliably sends: the @-mention and :emoji
  autocomplete menus no longer swallow the key when they have nothing to
  pick.** The suggestion menus' key handler claimed Enter/Tab even while their
  candidate list was EMPTY (menu mounted by an earlier query, then emptied —
  e.g. `:zz` mounts the emoji menu, appending a character empties its matches),
  and ProseMirror consults plugin key handlers before keymap plugins, so the
  send keymap never saw those presses: intermittent, silent, no error. The
  menus now claim nothing with an empty list (a pure, unit-tested decision
  shared by both renderers), and the send keymap additionally refuses any
  Enter that belongs to an active IME composition — belt to ProseMirror's
  suspend-on-compose braces. Verified: `bun run check`, `svelte-check`, and
  the full vitest suite green (1118 tests incl. 7 new composer-keys tests);
  the production build compiles; and the running surface was driven in a real
  browser via CDP — plain Enter sends once, Shift+Enter inserts a newline and
  the next Enter sends exactly once, a populated menu picks the highlighted
  row without sending (then sends once on the following Enter), a
  composition-Enter never sends, and a mounted-but-emptied menu falls through
  so Enter sends (this last case demonstrated FAILING before the fix, PASSING
  after).

- **A work session waits out a working agent instead of hanging up on it, and
  a dead session no longer strands the ticket.** A session turn is an agent
  driving its tool loop — clone, build, test, iterate — and honest dev work
  runs for HOURS; the step's wall-clock ceiling (11 minutes, sized for a
  different world) killed the session mid-turn while the agent was
  demonstrably working (last model call 67 seconds before the kill), and the
  ticket sat in_progress silent for seven hours because dispatch only fires
  when a ticket ENTERS a pickup column. Three changes: the runs engine gains
  an IDLE-BASED step deadline (each ping on the step's activity tap re-arms
  the clock; only silence abandons the step — the law the chat transport
  already lived by, carried into the engine), the work session adopts it at
  ten minutes of silence while the persona stream pings per chunk; and a
  `work-redispatch` sweep re-offers every pickup-column ticket with agent
  assignees to dispatch every minute, whose generation walk stands down on
  live sessions and advances on dead ones — a dead session now costs a
  minute of silence, not an evening. The dispatch prompt also tells the agent
  to sync its checkout to latest `origin/main` BEFORE working (`git fetch`
  alone does not move the working tree — the incident's agent was about to
  base work on an eleven-day-old tree). Verified: engine tests pin both idle
  halves (a pinging step outlives the wall clock; a silent one is abandoned
  at the idle ceiling with a sentence that names it), and the session def
  carries `idle_step_ms` by assertion.


- **A git credential the platform won't give fails fast instead of hanging the
  agent.** A push to a repo off the agent's grant list made the credential
  helper decline (correct), git fall back to its terminal prompt, and that
  prompt waits forever in a harness PTY — the agent reads as stuck, and the
  2026-09-14 incident was exactly this shape (the granted repo's pushes
  worked; the probe of an ungranted one hung). The render now sets
  `GIT_TERMINAL_PROMPT=0` in every agent service env, so every such case is
  git's immediate "could not read Username" — which the toolkit skill already
  tells the agent to `report_problem`. The helper's decline and the operator
  log both name the repo now (`no credential for github.com/owner/repo`), so
  the next stuck agent is diagnosable from one log line. Verified: an
  ungranted path through `git credential fill` in Doug's container fails
  immediately post-roll with the repo named in the stderr.


- **A parked tab stops burning a core — the standing wave fields now paint on
  a bucketed clock.** A dither wave field animates forever by design, and the
  engine repainted at full requestAnimationFrame for it: the daily brief's
  hero (always mounted on Home) alone measured **47.5 seconds of JavaScript
  per minute — ~80% of a core — on an otherwise idle tab**, which is the
  "browser slows to a crawl after Talaria is open for a while" report (memory
  stayed flat; it read as a leak but was continuous canvas paint). Waves now
  repaint at ~12/s on the same bucketed clock shimmer already used — a wave
  moves at single-digit pixels per second, a sub-pixel step per bucket, so
  the motion is visually identical — and tweens keep full rate, because they
  are short and they are the transition itself. Verified with a CDP profiler
  against the live instance: heap/DOM/listeners flat over navigation laps
  (no memory leak), and the burn attributed ~70% of samples to the dither
  engine's paint before the change.


- **An agent's `git push` authenticates again — the credential helper read a
  key the api never accepted and a URL nothing ever set.** The fleet-rendered
  git credential helper (`git-credential-talaria`) sent the chassis's own
  server key where `/api/secrets/git-credential` authenticates the agent's
  `tak_` credential — every helper call 401'd — and built its URL from
  `TALARIA_API_URL`, a variable no renderer ever wrote, so even a valid key
  would have posted to a relative address. The helper now sends
  `TALARIA_AGENT_KEY`, and the render bakes `TALARIA_API_URL` into every
  agent service env, derived from the app's own `TALARIA_GATEWAY_SELF_URL`
  (dev-defaulted to the docker host-gateway) — one env, so the agent's shell
  and the workbench harnesses running in the same container both get it. A
  failed fetch now says so on stderr instead of exiting silently: a
  credential that does not exist and a helper that cannot ask had the same
  silence. Verified: the helper's contract is pinned by render tests (the
  agent key, never the chassis key; the env-derived URL), and live on outcrop
  after the roll + a fleet reconcile the helper answered a real GitHub
  installation token for a granted repo from inside Doug's container.


- **A GitHub App private key pasted into the admin panel parses — the
  single-line input was stripping its line breaks.** The Key field was a
  password input, and the HTML value sanitizer deletes LF/CR from anything
  pasted into one, so a real `.pem` arrived (and was stored,
  envelope-encrypted) as `-----BEGIN RSA PRIVATE KEY-----base64…` on one
  line — `github app key parse: PKCS#1 ASN.1 error: PEM error: PEM type
  label invalid`, every time, with nothing visible wrong on the user's side.
  Two fixes: the field is now a textarea (multi-line paste survives
  verbatim), and the signer repairs a newline-stripped PEM at parse time —
  the stored shape is unambiguous, so installs already holding a mangled
  key start working without re-pasting. Verified: the stripped form of both
  PEM labels (GitHub's PKCS#1 and PKCS#8) signs the pinned JWT bytes
  (`api/tests/github_pem_repair.rs`), reproducing the exact reported error
  before the fix.

### Changed

- **A slow Muse draft survives the proxies between the browser and the api.**
  The structured kinds (the New Agent modal's design call, cron drafts,
  ticket patches) put nothing on the wire until the whole validated JSON
  exists — and a genuinely slow generation (outcrop 2026-09-14: a 2m20s
  agent design on a cold provider day) ran into Cloudflare's 100s idle
  ceiling and came back to the user as a timeout, while the api finished the
  turn for nobody. A draft that runs past 45s now opens its `application/json`
  body and drips `\n` heartbeats — leading whitespace is legal JSON, the
  client's parse cannot tell — ending with the same object the buffered path
  sends. Fast runs (the overwhelming case) answer byte-identically to
  before, statuses included; the answer's error split (400 configuration /
  502 model / 500 thrown) is pinned by test. The design modal also counts
  aloud now: past ten seconds the progress label shows elapsed seconds, so
  "working, slowly" stops reading as "wedged".


- **A re-drafted email converges on the approval already waiting, instead of
  queueing a twin.** A retried agent run re-drafts the same outbound email —
  reworded body, recipients reformatted — and every draft used to become its
  own p0 SEND EMAIL card: the outcrop brief held six cards for one reply to
  Cooper Beverage, and approving two of them was a double-send to a client.
  `queue_action` now matches a new gmail_send against still-pending rows by
  principal + recipient addresses + subject with the formatting boiled off,
  and returns the existing row with "an identical draft is already waiting"
  — nothing new queued, nothing announced twice. A decided row never
  matches: after a reject, a fresh draft is a fresh ask. Verified live: the
  re-draft from the incident report (display names vs bare addresses, flipped
  case, different body) collapses onto one pending row
  (`api/tests/pending_dedupe_live.rs`, house-ignored), and on outcrop the
  brief's approval dismissal lands and survives the sweep under #361's
  binary — the "can't dismiss" half of the report was the pre-#361 binary
  that ran until 15:14 UTC today.

- **The Files column headers breathe again.** The NAME/KIND/OWNER/MODIFIED
  band sat flush under the toolbar with bottom-only padding — two borders
  kissing and labels squeezed. It now carries its own margin from the
  toolbar and even vertical padding, so the toolbar / column heads / rows
  stack reads as three deliberate zones.

### Added

- **Files can manage Google Drive itself — rename, move, and trash, in place
  and on Google's side.** The personal connection's scope moves from
  drive.readonly to full drive (existing connections keep browsing read-only
  until one reconnect; the Drive place shows a "Reconnect Google to manage
  files" banner while that stands, and the write verbs stay hidden — a
  disabled verb reads as broken, a hidden one reads as not-yours-here). The
  org connection already carried full drive, so the Workspace Drive is
  manageable today; org writes require an admin, the same discipline the org
  connection's grant implies. Rename is the house prompt, Trash sits last
  behind the separator with a confirm (labeled Trash — Drive's trash is
  restorable), both on rows and on multi-selections. Drag-to-move works
  INSIDE the Drive place — folder to folder, onto the breadcrumb — routed
  through Google's addParents/removeParents, never Talaria's folders; a drag
  carries its source so a Drive drag can never hit the local move path. The
  Move dialog speaks Drive too: browses Drive folders, creates a Drive
  folder inline. Every write is audited (drive.rename | drive.move |
  drive.trash | drive.create_folder). Also gone: the no-op "Connect a
  source" rail row.

### Added

- **Google Drive is a place in Files, browsed like every other place — the
  import modal is retired.** The rail's Sources row opens the Drive place:
  the same list/grid columns, the same selection and keyboard grammar,
  breadcrumbs into Drive folders (walked server-side so a deep link rebuilds
  its own path), server-side sorting (honest with pagination), and Load more
  past the first hundred. The rail expands the ROSTER while you're in the
  place — one row per browsable Drive, visually distinct: your My Drive,
  shared drives your account joined, and the org connection's Workspace
  Drive marked `org` — so personal and org files can't be confused. The URL
  carries the selection (`/artifacts/drive?d=<drive>&f=<folder>`), the same
  URL-is-the-selection rule as everywhere. Import is a first-class verb now:
  select Drive files, Import, and they land in a My Files folder named for
  the Drive folder they came from — sequential pulls (Google rate limits),
  one summary toast, oversize files counted. New: GET
  /api/integrations/google/drive/drives (the roster; 409 only when BOTH
  connections are absent — that 409 is the Drive place's connect screen) and
  GET .../drive/browse (folders included, paginated, with the walked path);
  the import route gained `folderId`. The old flat /drive/files search route
  is gone with its modal; the agent drive route is untouched.

### Added

- **Files learns to move, copy, and duplicate — the whole management
  grammar.** Cut/copy/paste for files AND folders: ⌘X/⌘C/⌘V, the selection
  bar's Cut/Copy/Move, row and breadcrumb context menus (Paste into), and a
  Paste entry on right-clicking empty space. Cut dims the spoken-for rows;
  Escape clears the clipboard before the selection (the more transient thing
  first); a cut pasted back where it came from is a silent no-op while a copy
  pasted in place duplicates — Drive semantics. Copy's engine is new on the
  server: POST /api/artifacts/{id}/duplicate and
  /api/artifact-folders/{id}/duplicate copy one record or a whole folder tree
  in one transaction (recursive CTE, cycle-guarded). Copies are the caller's
  and private, land beside their source under a "Copy of" name with a
  bounded collision ladder, share the source's storage blob (safe — no
  delete touches the blob), and never inherit public slugs, KB mirrors, or
  Google linkage. And the Move dialog is finally here: a navigable folder
  browser with breadcrumbs, a New-folder control, and self/descendant
  destinations dimmed and refused.

### Changed

- **The Files view's selection is a first-class column, and the keyboard is a
  first-class citizen.** The row checkbox used to float over the kind icon and
  swap with it on hover — hovering a folder made it a checkbox and hid what the
  thing WAS. Selection is now a permanent leading column: every row shows its
  checkbox to the LEFT of its icon, always visible, and the column header
  carries a select-all checkbox (⌘A's visible twin). The keyboard speaks the
  desktop file-manager grammar: arrows move focus and select (Finder/Explorer
  style), Shift extends from the anchor, ⌘/Ctrl+arrows move focus alone,
  Space toggles, Enter or → opens, ← climbs a folder level, Home/End jump;
  the grid view steps by tile instead. Rows carry a visible focus ring, focus
  heals to the nearest surviving row after a delete, and modifier-clicks on a
  row body select instead of opening. Verified end-to-end in a browser:
  icons never disappear, select-all agrees with ⌘A, shift-ranges extend from
  checkbox and body clicks alike, and every key above does what it says.

### Fixed

- **Fresh deploys no longer die pulling minio — MinIO removed its Docker Hub
  namespace, and every minio image reference now points at quay.io, which
  hosts the same builds.** Symptom, from a customer deploy: `pull access
  denied for minio/minio, repository does not exist or may require 'docker
  login'` — verified upstream from two independent networks: the Docker Hub
  repo 404s (the whole `minio` namespace, `minio/mc` too) while
  `quay.io/minio/minio:latest` is digest-identical to the last Hub-published
  build and pulls fine. Swapped `docker.io/minio/minio` → `quay.io/minio/minio`
  in the deploy, dev, and devbox compose files, and `minio/mc` →
  `quay.io/minio/mc` in `talaria box seed` and the backup sidecar's image
  default — that last one would have broken existing installs' backup jobs on
  their next mc pull, not just fresh deploys. Verified: `bun run check` green
  (gen-docs current), tests 1118/1118, typecheck clean outside the gitignored
  client subrepos (known local-only leak; CI is truth), all three compose
  files parse via `docker compose config -q`, and both quay images pull on
  this box (mc build 2025-09-07; `TALARIA_MC_IMAGE` still overrides).

- **A wedged request no longer hangs a view until the page is reloaded.**
  The agents roster reported it — often on first load, the skeleton stayed
  for ever and only a refresh helped — but the hole was app-wide: browsers
  give `fetch` no timeout, so a request whose connection silently died (a
  dropped keep-alive after a server roll is the observed shape) stayed in
  flight for ever; the query above it never errored, so it never retried,
  and the surface waited on a skeleton nothing would ever claim. Two
  deadlines close it, both READS-ONLY by the same rule. Every read through
  the app's one HTTP door (`fetch-json`) now carries a 30s abort — a wedged
  read becomes an error TanStack Query retries, so the roster heals itself
  instead of waiting for a person to notice — and the SPA host's proxy to
  the Rust api carries a 30s deadline to FIRST RESPONSE on GET/HEAD: the
  stale-keepalive race a container roll leaves behind now answers a 502 the
  retry can act on. Writes are deliberately exempt everywhere — fleet verbs
  and reconcile block on docker for minutes, chat completions and assistant
  tool-call commands stream via postStream, uploads POST their bodies — and
  the proxy's deadline disarms the moment headers arrive, so the SSE relay
  and every other streaming body run unbounded. Verified end-to-end on the
  dev stack: with the api frozen mid-request (SIGSTOP), a proxied read
  answers 502 at the deadline and 200 in 15ms the moment the api resumes;
  the fake-timer tests pin all three halves (never-answering read → 502; a
  POST that never answers stays pending past ten minutes of fake time; a
  streaming body outlives the deadline).

- **The knowledgebase's first-ever landing stopped swallowing doc clicks.**
  Land on `/knowledge` with no saved selection and the sidebar rendered the
  first space's tree — but every doc click was a no-op, until you opened a
  second space and came back. Two defects, one scene. The canonicalising
  effect (put the first space in the URL) had returned at `!restored` on its
  first run, and a plain `let` latch can never re-run an effect: on a landing
  with nothing to restore, nothing ever changed the URL, so it stayed bare.
  And on a bare URL a doc click called `setLoc(null, doc)` — which navigates
  to that same bare URL. The latch is `$state` now, so the effect wakes when
  the restore answers, and the restore's answer (`restoredSpace`) keeps the
  first-space default from racing a real saved selection. The doc click
  names its own space — the tree renders under the active space whether or
  not the URL carries one. Verified end-to-end in a browser: first-ever
  landing canonicalises to the first space, a saved selection still restores
  to ITS space, a deleted space's memory still falls to the first space,
  and a doc click on a fresh landing opens the document.

- **Chats stopped 500ing — every conversation read, on every instance, since
  the rooms cutover.** The cutover's migration said "every reader of
  `tasks.conversation_id` was re-pointed before this drop," and six readers
  were not: the conversation access predicates, the prior-turn transcript,
  the notification audiences, and the task-delete unindex all still joined
  the dropped column. Postgres validates a statement's whole text when it
  plans it, so the mere presence of the dead reference broke EVERY
  conversation detail and history read — ticket branch or not, chat or plan
  or research — which is why a threads sidebar looked fine while opening
  any thread answered 500. The dead legs are gone (no `kind='ticket'`
  conversation can exist since the cutover deleted them all; a task's
  thread is a channel room, gated by board membership on the channels
  side), and the delete-task unindex is re-pointed to the room's
  `channel_messages` — since the cutover it errored silently, leaving a
  deleted ticket's comments answering searches. Verified against a
  post-cutover database: the detail route answered 500 before the fix and
  200 after, same row, same session.

- **The boards list view's columns sit under their headers again.** Every
  row carried the hover dither field (`dither-fill`), and the field's
  canvas was inserted as a DIRECT child of the `<tr>` — but a table row
  admits only cells, so the browser wrapped the stray canvas in an
  anonymous cell of its own: a whole phantom column ahead of the checkbox,
  walking every real column one to the right of its header. The header said
  Status over nothing, the status pill sat under Priority, priorities under
  Assignees, and the timestamps hung past the last header with none at all.
  The canvas now hosts inside the row's first cell — it is absolutely
  positioned against the row either way, so the full-row field is unchanged
  (verified: 0 painted cells at rest, the full row painted on pointerenter).
  One fix, every table that carries the field on a row: board list rows,
  the fitness matrix's rows, and the public artifact tables.

- **A cross-off no longer comes back with tomorrow's date on it.** The brief
  is one document per day, and a check-off lived only inside the day's own
  append-only log — so every morning's open re-listed everything still live
  in its sources, including the rows the owner had crossed off the day
  before, un-crossed, forever. The owner's verdict now carries: the open and
  the sweep skip a key whose newest entry on a prior day is the owner's own
  check or dismissal with an unchanged source fingerprint, for thirty days.
  The item reappears the moment its source actually moves (the task fails
  differently, the thread gets a new message, the draft is rewritten) — the
  same rule the within-day sweep always closed lines by — and `restore`
  still works from the prior day's page, which the read serves before the
  day's own brief opens. Verified against the live database
  (`api/tests/brief_verdict_carry_live.rs`, ignored like every live proof):
  a checked-off blocked task is absent from the next day's document, a
  stale-fingerprint verdict does not suppress, and a restore lands on the
  prior page and is re-added by the next sweep.

- **Approvals are decided on the line, not by asking around them.** A
  pending outbound action — an email or an event the assistant drafted —
  appeared as a p0 "Needs you" line that went nowhere: its href was a
  placeholder `/`, and no page in the app listed pending actions at all, so
  the only way to act was a conversation with the assistant. The line now
  carries its own decision block, quoting the exact outbound payload (who
  receives it, what it says, who drafted it) above Approve/Reject buttons —
  the same contract the drafted-reply block keeps, which is why neither
  carries a modal. The buttons ride the existing
  `/api/integrations/google/pending/{id}` decide route; the sweep reads the
  pending row's decided status and closes the line as APPROVED or REJECTED
  rather than a generic DONE. Verified on the running stack: the line
  renders with its payload, a reject closes it as REJECTED on the next
  read.

- **A mention in a plain channel lands on the channel, not the channel
  index.** The mention notification's href was `/channels` — a page that
  now redirects to the comms root — so the reader arrived nowhere specific
  and went looking for the conversation themselves, which is the work the
  notification was supposed to save. It now carries
  `/comms/channel/{id}` like every other channel-shaped notification, and
  the brief's accessibility check passes it for the members it is for.
  Verified by posting a live @mention through the real route.

- **Agent-problem notices reach the brief at all.** The fact-flavored
  agent-problem notifications (the ones telling workspace admins a problem
  exists without quoting it) carried no href, and the brief lists only
  notifications with somewhere to go — so they showed in the bell and never
  on the page that is supposed to be the day. They now point at
  `/observability/alerts`: never at the filed helpdesk ticket, because the
  fact audience is precisely the people not on that board, and board
  visibility is membership rather than role — a ticket href would be
  dropped for every reader the row is for.


- **Google sign-in no longer dies as ERR_TOO_MANY_REDIRECTS.** The OAuth
  relocation (added with the pinned-origin rule) answered "relocate to the
  pin" whenever the derived origin string differed from `AUTH_PUBLIC_URL` —
  and when the difference was a LIE the proxy chain told, the redirect
  pointed back at the URL the browser was already on: follow it, resend the
  same headers, derive the same "wrong" origin, redirect again, until the
  browser's budget died. The lies that triggered it are ordinary deployment
  shapes: an outer proxy that states the host but not the scheme (the app
  host backfills `x-forwarded-proto: http`, and an https pin never equals
  an http hit), an explicit `:443` on the forwarded host, a typed-uppercase
  pin, a comma-joined proxy chain. The one-origin check is now a HOST
  check — cookies (the thing the relocation protects) are host-scoped, and
  scheme and port never partition the jar, so a browser already on the
  pinned host is already home and no redirect could have helped it. A
  genuinely different host (the LAN URL the pin exists to retire) still
  moves home in one hop, path and query intact; all five start/callback
  routes share the one fixed function.

- **The inherit path's edge-boot helper was born blind to docker.** The
  helper runs the app image as its own non-root user, and its `docker run`
  carried no `--group-add` — so the 0660 root:docker socket denied its
  every call. The first shape's `docker inspect … 2>/dev/null | grep -q
  true` read that denial as *the old container already stopped*, the wait
  skipped, the compose refused, and the helper exited before the port ever
  freed: the old container stopped itself for the one-time cut, the edge
  never rose, and nothing served the port (a client VPS took exactly this
  outage). The helper now carries the same host group ids as the app
  container, and its script treats an unreachable daemon as a loud failure
  with a breadcrumb — `edge-boot.log` in the update dir — instead of a
  confident answer it never had.

- **A stranded cutover waited hours for a reader that could never come.**
  `reconcile_boot` is the only writer that finishes a cutover, but its
  callers were the 6h check (whose interval lease a dead container's ghost
  holds for the whole interval) and the admin read — which arrives through
  the very edge a stranded cutover has not raised; green publishes no port
  of its own. And when reconcile *did* run while the old container still
  held the port, the edge-raise lost the bind race and the whole reconcile
  errored instead of recognizing the hold. A new `update-reconcile` job —
  one reconcile a minute, one settings-row read when idle — now retries
  the handover however it stranded, and the hold with the port still held
  answers as the hold it is: the edge rises at the cut, and the next tick
  lands the run.

- **The stale-close fired on live adoption holds — and its wrong closes
  never healed.** A run in flight past an hour closed as failed even when
  *this container* was the run's own retired orchestrator, alive and
  serving the hold: the canary sat eighty minutes at a green adoption
  hold, both doors 200, and the row said failed. The exemption is the
  retirement itself — that run is ours and we are its proof of life. And
  for rows the close already marked failed while the containers finished
  the handover anyway, the reconcile now heals them to `done` when it can
  prove the landing (this container is the rolled-to digest, nothing
  retired is running, and the edge answers) — by hand no longer.

- **The @ picker froze at editor creation — and ticket threads never
  offered agents.** MentionSuggest captured the mentionables array at
  mount, but every surface builds that list with `$derived` over async
  queries: the picker showed whoever had resolved by editor creation
  (cached users, typically) and never the agents that land with the
  fleet — and a list still empty at mount installed no picker at all,
  leaving the @ key dead. Candidates are read live on every keystroke
  now, and a ticket's discussion offers the board's agents ahead of its
  members, addressed by label — the same grammar the channels read. The
  description editor stays humans-only: a mention there notifies a
  person, and no agent reads a description.

- **An embedded chat fills its container; the window floats over it.**
  ChatView's root was transparent — invisible on the full-page stage,
  where the page behind it is the same ground, but everywhere the chat is
  embedded (a ticket's discussion tab, the plan split, a research run) it
  sat on panel chrome, and the composer's opaque ground band floated on
  that chrome with no ground of its own: a dark patch with seams on three
  sides, disjointed from the surface around it. The root now paints the
  ground, so the container becomes the chat's stage, edge to edge, and
  the transcript column and the composer float over it — the geometry the
  full-page stage always had.

- **The edge container's image tag never existed.** The pinned default
  `traefik:v3.6.7-alpine` was a tag nobody ever published — the 3.6 line
  tops at `v3.6.25` and carries no `-alpine` variants — so adoption's
  first fleet run brought green up and then silently never started the
  edge at all. The default is now the real `v3.6.25` (the plain tag is
  Alpine-based and carries the busybox `wget` the edge healthcheck needs).

- **The adoption's live stage tells the truth about a dead edge.** The GET
  answered `edge-ready` whenever the state row carried a port and blue was
  still serving — a condition the first fleet adoption actually reached
  with the edge container down (the bad tag above): a poller sailed past
  the hold and repointed a proxy at a port nothing answered on. The stage
  is now derived against the containers themselves: the edge running is
  `edge-ready`, edge_port set but the edge down is `edge-pending`, and the
  panel shows it with a Retry button (the adopt POST on blue replays the
  edge's compose and waits — it cannot cut over, so no confirm).

- **The update dir lives under the state root, not a baked absolute path.**
  The image baked `TALARIA_UPDATE_DIR=/var/lib/talaria/update` — outside
  every deployment's bind (fleet VMs bind only their
  `/var/lib/talaria-<customer>` state dir), so the updater-owned compose
  and slot env files landed in the orchestrator's writable layer, gone on
  its next redeploy and invisible to a host-side inspection. The default
  is now `${TALARIA_STATE_DIR}/update`, the Dockerfile stamp is gone, and
  the variable is denylisted in slot renders so a stale baked value cannot
  propagate into the project it broke.

- **The code probe's clock starts at the candidate, not the engine.** The
  fitness code tasks ran inside a 250ms window that also covered building
  the boa context evaluating them — on a loaded host (CI's runners proved
  it) a cold context build ate the whole window and a correct solution
  failed with "the code did not run". The evaluator now signals when the
  engine is built and the window times only the candidate's script;
  an infinite loop still costs its 250ms exactly as before.

### Added

- **The fitness suite measures what it claims, and an admin can see when it
  is the suite that is wrong.** A sweep of the model-test results on the dev
  and dogfood instances found a ton of gaps that were ours, not the models' —
  checkers grading one wording of an answer they claimed to grade by
  substance, fixtures that could not fail, and bugs wearing model names. Four
  tranches, all landing here. **The health band is loud**: `ours` — fixtures
  every tested model got wrong, which is a bug in the fixture wearing a
  model's name — is stamped into one cached settings row whenever the archive
  changes (a run finishing, Clear, Forget) and rides the matrix payload the
  default tab already polls, so a poll stays one small row rather than a
  re-fold; when it is above zero the matrix shows a line saying so, and the
  line is a door to the health tab that names the fixtures. **New fixtures
  grade behavior prose could not see**: the concluder must prefer the
  benchmark it actually ran (concluder "benchmark"), the briefer must carry
  the numbers forward rather than cite the text it was summarizing (briefer
  "ledger"), a knowledge answer must surface the doc that was found
  (hermes_knowledge title find), a spent secret handle must not be presented
  as still spendable (secret_handles spent-position), and the long-context
  needle is planted fresh per run so a cached answer cannot pass
  (long-context fresh needle). **Graded behavior is stated where the model
  reads it**: the Google harness states the calendar-conflict rule in its
  system prompt and carries a silent-move trap for it; the muse taxonomy
  grades refusal-vs-downgrade and is pinned by test; research plans can
  discover prior runs. **The vision probe draws a shape**: solid colour
  fields only sample a tint — a fourth trial (black square on white, named
  against circle and triangle) is answered only by parsing spatial structure,
  and the answer is in the pixels and nowhere else, so nothing in the prompt
  lets a non-reading model fake it. **report_gap's kind is graded as the
  contract states it**: the work-session gap fixture now fails a kind
  written as a sentence or filed as "misc", because repeats are matched on
  the slug and a sentence per phrasing means recurrence never accumulates.
  Verified: 1952 lib tests green (surface 94, defs 396, probes 84, toolbox
  78 among them), `bun run check` and svelte-check clean; the health band's
  four tests cover stamp-on-run, restamp-on-clear, restamp-on-forget, and
  the matrix reading the cached row without folding the archive.

- **Adoption — the one-time handover that turns the engine on.** An admin
  (or the migration script, through the machine key) presses Take over
  updates and the install stops being orchestrator-deployed: adoption pulls
  the image it should be (the registry's newest, or a re-pin of the running
  image's own digest when the registry is silent), renders the
  updater-owned project from the live container's own inspect document,
  brings slot A up behind the same health gate a roll uses, moves the fleet
  alias, and records the handover crash-safe before any port changes hands.
  Then the port decision: a fresh port (the infra path) puts green and the
  edge up on a port nobody holds while blue keeps serving — the second
  adopt call, which can only arrive THROUGH the edge, is the proof the
  proxy moved, and it stops blue; zero interruption at the origin. The
  inherit path (the panel's button) takes a one-time cut of a few seconds,
  named in the confirm dialog: the dying container arms a host-side helper
  (the app image itself, docker-cli baked in) that raises the edge the
  second the port frees, with green's reconcile as the crash fallback.
  adopt on an adopted install is the resume — every crash heals on the next
  call. The retired container is stopped by the finish, removed by the
  reconcile, and nothing resurrects it. Still true everywhere else: an
  install that never adopts keeps deploying exactly as it always has.

- **docs/UPDATES.md — the update story in one place.** The taxonomy (who may
  roll and why the refusals say what they say), digest pinning (a roll never
  pulls a moving tag), the roll step by step including the two-process split
  and the never-released lock, rollback (panel and the docker-only runbook
  for a dead instance), the deploy key, the two-switch auto-update story, and
  the overlap contract — the expand-contract migration rule a contributor
  must keep, now linked from CONTRIBUTING.md's rules.

- **The update panel rolls with the engine — and the git-checkout updater
  retires.** Admin → Security → Updates now answers against
  /api/admin/updates: running version and slot, what's available, Check /
  Update now / Roll back (the kept old slot is the target), the auto-update
  toggle (image installs only, off by default), and the deploy key — minted
  once, shown once, stored as a hash. Non-image installs show the engine's
  own refusal sentence; un-adopted installs keep deploying the way they
  always have, and the panel says that too. The TS git updater it replaces
  (ui/src/server/updater.ts, the admin.update resident,
  scripts/update-restart.mjs) is deleted — the proxy resident list drops to
  three, and the routes/api reference carries the Rust surface instead.

- **The update engine rolls — and its panel and schedule arrive with it.**
  One update, start to finish, split across two processes by design: the
  live container gates (mode, adoption, nothing in flight), takes the
  fleet-wide roll lock (a redis lease held by heartbeat and never released
  — the roller's last act stops its own container, so the TTL is what
  bounds a dead one), pulls the digest-pinned image, renders, brings the
  other slot up, health-gates it (the compose healthcheck IS the gate — an
  unhealthy replacement is removed with the old container never disturbed),
  moves the fleet alias, and only then records cutting-over and stops
  ITSELF; the new container's boot reconcile verifies the edge actually
  routes to it before the run lands done. Rollback restarts the kept old
  slot by the same choreography, and a run whose roller died mid-roll is
  closed as failed so no install spins forever. `/api/admin/updates`
  surfaces all of it (running version + slot, available, history, the
  auto-update toggle — default off) for an admin session or a per-instance
  machine key minted for the deploy script and stored only as a hash.
  The update-check job rides the scheduler table again (6h cadence; the
  hold is retired) as the hands-off half: resolve, record, and — only
  behind both the adoption and the toggle — roll. healthz now reports the
  image's version so a deploy gate can tell two instances apart. Still
  dormant everywhere: the engine acts only where an admin adopts it.

- **The update engine learns to render its slots.** The updater-owned
  compose project now has a shape: one derived from the LIVE container on
  every roll (env verbatim minus a denylist — the new image's version and
  install signal must win, and the updater kill switch is dropped because
  adoption is what flips it — same-path binds, docker gid, dns, the
  sidecar network joined as external so it stays owned by the project that
  runs the sidecars), two flip-rendered app slots with the compose
  healthcheck carried over byte-identical (the edge's docker provider
  refuses starting/unhealthy containers — that check IS the roll gate) and
  identical traefik labels so both slots merge into one health-gated
  service, and a pinned traefik edge that owns the host port the slots no
  longer publish. The docker verb set it drives ships beside it
  (inspect-self, per-service slot up, digest pulls, health waits, graceful
  stop-not-remove, alias attach). The image stops baking
  `TALARIA_UPDATER=off` — it contradicted the install signal (an adopted
  slot would have inherited it through the image's own env and the engine
  could never act); dormancy is the adoption gate, and operators who want
  the hard switch set the env themselves. Still nothing routes to any of
  this: no behavior change for any running install.

- **Agents developing Talaria get their own floor.** `AGENTS.md` is the
  canonical instruction file for anyone — human or agent, on any harness —
  coding in this repo: the distilled rules, the command map, environment
  facts (ports, worktree isolation), parallel-session etiquette, the skills
  index, and the do-not-touch list, kept under ~200 lines because it loads
  into agent context at launch; `CLAUDE.md` is a stub importing it, since
  Claude Code reads `CLAUDE.md` while opencode, Pi, Oh My Pi, and Codex read
  `AGENTS.md` directly. Four skills (`.claude/skills/`: dev-loop, repo-traps,
  ship-a-change, cut-release) carry the procedures — frontmatter `name` +
  `description` so Claude Code and opencode both discover them natively, with
  the AGENTS.md index as the path for everything else — including the
  operational traps that cost real time, each phrased symptom → check → fix
  and verified against the code they describe (the session-mint field order
  against `api/src/session.rs`, the scheduler kill switch against
  `api/src/scheduler.rs`). The stop gate is a harness-agnostic library:
  `scripts/hooks/stop-check.mjs` runs `bun run check` (seconds, zero
  dependencies) under one contract — exit 0 silent pass, exit 2 block with
  the reason on stderr, anything else a non-blocking error — wired into
  Claude Code by a tracked `.claude/settings.json` holding only the Stop
  hook; permissions stay personal in the untracked `settings.local.json`,
  which the tracked `.gitignore` now covers along with `.claude/worktrees/`.
  How the layers fit and how each harness consumes them is documented at
  `docs/AGENT-TOOLING.md`. Also corrects the stale "frozen at cutover" notes
  about `docs/api/` in CONTRIBUTING.md and DEVELOPERS.md — the #293 extractor
  landed; the reference is generated. Verified: `bun run check` green from a
  checkout with no `node_modules` (every new link resolves, no generated
  drift), the hook standalone passes silent and exits 2 on a planted dead doc
  link, `git check-ignore` proves both new ignores, and Claude Code loads the
  stub and lists the four skills (`/context`, `/skills`).

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

- **CI publishes the app image from main.** Every push to main that touches
  running bits now builds the full app image and publishes it as
  `ghcr.io/outcrop-labs/talaria:sha-<sha12>` (immutable, first) and `:main`
  (pointer, second) — trunk images, beside the release channels a human
  cuts. The api stage is digest-pinned before each build: an api-touching
  push waits for that same commit's api package (built in parallel by
  api-package's own run), anything else pins the digest
  `talaria-api:main` names — a trunk image never mixes a new UI with an
  unbuilt api. No behavior change for any running instance: the feed is the
  groundwork the in-app rolling updater consumes; `TALARIA_INSTALL=image`
  now ships in the image's env as the signal that updater will key on
  (absent in checkout installs, where it stays dormant).

- **The browser tab carries the instance's company name.** Admins can name
  the workspace (Admin → org tab, beside the hosting domain); until then
  the tab reads "Talaria", and once named it reads "Talaria - <name>" — on
  every route and the sign-in page too, because the name rides the public
  identity beacon alongside the instance id rather than an authed read.
  Purely cosmetic: nothing else derives from it. The name trims on save,
  whitespace-only is refused (clearing is an explicit null an accidental
  save can never perform), and every set lands in the audit log.

- **Notifications reach a browser that's closed.** Web Push, end to end: the
  settings' desktop-notifications toggle now also files the browser as a push
  recipient (a per-device subscription — the push service's endpoint plus
  the key halves the payload is encrypted against), and any notification the
  single writer files with an in-app copy fans out to every subscribed
  browser of that person. The payload travels as RFC 8291 `aes128gcm`
  ciphertext under RFC 8292 VAPID proof — hand-assembled on `p256` rather
  than the OpenSSL-carrying `web-push` crate, keeping the runtime's
  no-OpenSSL invariant, and pinned to the RFCs' own test vectors. The
  instance's VAPID keypair is born once and its private half sealed under
  the instance secretbox. The service worker shows the OS notification only
  when no Talaria window is visible (the toast already covered the open
  case) and lands you on the notification's target when clicked; dead
  subscriptions (the service answered 404/410) prune themselves, and turning
  the toggle off retires the subscription at both ends. Classes routed
  **Email only** still reach mail alone.

- **An agent reply that lands while you're away rings once.** Replies in
  agent chats, plans, and research discussions now file a single
  "`Dex` replied" notification per person whose read cursor doesn't cover
  them — watching the thread type counts as reading it, so the notification
  is for whoever genuinely wasn't there. One row per thread: further replies
  fold into it until it's opened, exactly like a DM. The row points where
  the reply lives (the thread, the plan, or — for a research discussion —
  the run, so opening the run clears the bell and the badge together), and
  rides the dm class: toasts, the bell, email, and the brief all pick it up
  through the one writer every other notification uses.

- **The sidebar carries live unread counts for Comms, Plan, and Research.**
  One endpoint (`GET /api/unreads`) sums what's waiting in each part of the
  app — rooms plus agent chats under Comms, shared-plan turns under Plan,
  unseen research runs under Research — using the same predicates the pills
  on each surface show, so a badge can never disagree with the pills it
  summarizes. The counts move live over the shared event stream and
  self-heal on a 30-second floor; a failed read renders `!`, never a silent
  0. Opening a research run is the seen gesture: its notifications clear
  with it, and the badge and the bell empty together.

- **Rooms and threads ring the rail without being opened.** A message in a
  channel you're not looking at, a reply landing in an agent thread you walked
  away from, a run finishing while you read email — all of it now moves the
  interface the moment it happens, on whatever page you're on, over a single
  event stream per tab (two used to open: one for toasts, one for the brief —
  each a dedicated server subscriber).

- **Agent threads and plans finally track what you've read.** Conversations
  grow a per-person read cursor — one table for agent chats and plans alike —
  so a reply that lands while you're away grows a count pill on the thread's
  row (agent chats in Comms, plans in the Plan rail), and opening it clears
  the pill. Your own turns never count, a teammate's turn in a shared plan
  does, and the cursor only ever moves forward. Right-click a thread →
  **Mark read** clears it without opening.

- **The bell: your notification inbox, finally openable — and clearable.** A
  bell in the top strip counts what's unread (a failed read shows `!`, never
  a silent 0) and drops the latest notifications: click a row to go there
  and mark it read in one motion, or clear everything with **Mark all read**.
  Until now nothing ever marked a notification read — the count could only
  grow. Marking by href (everything pointing at the page you opened) joins
  marking by id, and the inbox brief updates with the bell so the two can't
  disagree.

- **Agent-made things belong to the human behind the run — the responsible
  user, not the agent.** A knowledge doc or space, a document, file, or saved
  image, a research report, a workbench plan: whatever an agent creates now
  carries an owner, resolved by an attribution ladder — a personal
  assistant's owner; the human an org agent is answering mid-chat (the same
  turn key research reports back to); otherwise the admin who hired the
  agent; and an untraceable agent stays ownerless, as before. The owner gets
  the ordinary human's controls — share, make private, set policy — replacing
  the old answer where org-agent output was locked to an allow-list nobody
  could sit inside. Default visibility is deliberately unchanged: an org
  agent's output stays workspace-readable (ownership adds control, not
  secrecy), a personal assistant's stays private to its owner, and an org
  agent's research stays org-visible and ambient-indexed however owned —
  org-ness is derived from the run's own row (`requested_by` = the model,
  plus the agent being ownerless in `agent_defs`), never a serialized flag,
  so runs in flight across the deploy resolve correctly. The artifact
  listing also caught up with the single-artifact GET: a personal assistant
  now sees its owner's private files in the Files browser it could already
  open them from. Going forward only — existing ownerless items stay as they
  are.
- **Personal assistants inherit their owner's reach for read + draft — with the
  grant friction gone and introspection in its place.** Boards were the one
  policy-gated hole: the board listing showed an assistant its owner's boards
  while the board interior 403'd it, and the only path onto a board was an
  editor's hand. Now a personal assistant adds *itself* to any board its owner
  can read (`POST /api/boards/{id}/agents/self` — one step, own row only,
  audited), removes itself the same way, and for boards the owner cannot see
  files a request (`/api/boards/{id}/agent-requests`) that lands in the
  editors' approvals queue as a sixth approval kind (`board_access`) with a
  one-press Approve/Decline card in Board settings → Agents — approve grants
  and closes atomically, decline notifies the requester's owner. The board's
  agent policy stays authoritative throughout; nothing here widens it.
- **`GET /api/agent/whoami`** — the introspection call agents were missing:
  identity (personal? whose? elevated?), every board it can work with *why*
  (policy / owner / elevated), channels, MCP servers, the guardrails that
  will refuse it, and its own pending access requests — plus the MCP tools
  that act on the answer (`whoami`, `join_board`, `leave_board`,
  `request_board_access`). The fleet verifier's auth oracle moved onto this
  purpose-built route from its old borrow on `GET /api/users`, so narrowing
  the people directory can no longer take the toolkit dark.
- **A personal assistant reads its owner's private knowledge.** `can_read_agent`
  gained an owner arm: private docs, spaces, and artifacts the owner owns open
  to their assistant across the file plane (doc GET, doc trees, space and
  artifact listings, Drive export) — the brain already retrieved that
  collection for them, so two planes had stopped agreeing about "it reads your
  private work". Edit stays grant-only; sharing, routing, and officialness
  stay human-only.

### Changed

- **Migrations ship in the image — and bad ones die before (or at) the
  door.** The `MIGRATIONS` array is now the stated single channel for schema
  changes AND one-time data operations (a backfill, a watermark reset, a
  repair): append a statement, redeploy, done — running psql by hand on a
  customer database is never part of a release, because a fix that lived only
  in a shell history did not ship. Two CI gates back the law: a new
  `migrations` workflow boots a scratch `postgres:16-alpine` (the exact image
  the sidecars run), replays the entire array from zero, runs the pass a
  second time requiring `applied: 0`, and diffs the resulting schema against
  a committed snapshot (`ui/src/server/db/schema.snapshot.sql`, regenerate
  with `bun run migrations:snapshot`) — so every schema-touching PR carries
  the real schema diff as its review artifact, and a statement with a syntax
  error or a bad column reference can no longer pass CI and explode only at
  customer boot. The invariant check adds a destructive-statement guard: any
  statement that can destroy or rewrite data (drop table/column/type,
  truncate, an unscoped delete or update, an `alter column … type`, a
  rename) fails CI unless it carries an inline `-- deliberate: <why>`
  comment — the marker is the second look, recorded next to the SQL it
  justifies. And a FAILED boot migration pass now reports `migrations: !ok`
  on `/api/healthz` (503), flipping the compose healthcheck unhealthy
  instead of a green container whose every table query 500s; slow-but-running
  passes stay non-fatal, because slow is not failed. The docs that claimed
  the array was frozen and sqlx owned new migrations — never implemented —
  now describe the array as the live single channel it has always been.

- **The fleet stops queueing on its own database.** Nothing in the API rate
  limits agents — but four mechanical throttles made parallel load *feel*
  throttled, and all four are gone. The pg pool grows from the inherited 10 to
  40 (`TALARIA_PG_POOL_MAX`, clamp 1–200) with a 15s acquire timeout
  (`TALARIA_PG_ACQUIRE_TIMEOUT_MS`) so bursts queue instead of 500ing at 5s;
  the UI's own pool moves from a hardcoded 10 to `TALARIA_UI_PG_POOL_MAX`
  (default 20), both wired through compose's environment block (api 40 + ui
  20 + migration 1 = 61 of the sidecar's 100). A warm agent turn — the whole
  reason for the Rust port — stops paying ~7 serial pool checkouts before the
  upstream call: the endpoints table serves from a 15s window (every
  in-process writer drops it), the endpoint key decrypts once per 60s
  (rotation invalidates), `tlk_` identities resolve from a 15s cache that
  negative-caches unknown keys (revoke/policy edits reset it in-process), and
  the gateway's four hot `app_settings` reads serve from a 15s layer that
  `set_setting` invalidates. The OpenRouter US-pool fetch is single-flight
  with a 60s failure backoff — one TTL lapse under N concurrent turns makes
  one external fetch, not N × 10s hangs — and the shared HTTP client gains a
  10s connect timeout. healthz reports `pgPool` (size/idle/max) and warns when
  the pg ping lands slow with the pool drained, so future saturation is
  visible rather than guessed at. And the Inbox's per-user turn lock comes
  off the panel's own reads and state writes: GET /conversations/{id} serves
  mid-turn (MVCC snapshots can't read half a write; the page's `working` flag
  renders the in-flight reply) and a snooze mid-stream is a state change, not
  a conflict — the lock stays on command and action POSTs, where seq
  allocation and decision execution actually race.

### Fixed

- **Nine checker bugs that graded our gaps as model failures — and one
  guard widened only after its own trap test vetoed two tokens.** The
  sweep's headline fixes: the json-strict gate's verify step tested closure
  the wrong way round (a strict failure looked verified), and unstamped
  pre-revision records now self-heal on read rather than failing every model
  forever; the workbench's stale-grade regex matched "NOT FINISHED" grades
  as processed (`NF[KD]?D` on a grade that also reads "NOT DONE"), with a
  scripted repro pinning it; the judge gate rework carried both. The
  research gap-round checker now catches the question re-asked as a query
  by token containment (all but ≤4 of the question's own words) — the
  commonest repeat matched neither literal and re-trod the findings for
  free, while a query naming new ground cannot carry those words and stays
  legal. Six too-strict alternations widened, each anchored on words that
  survive paraphrase and each with a trap test: a deferral recorded as
  "postponed"/"no longer" no longer fires plan_doc's both-positions check
  (the widening dropped `revisit` — CURRENT_DOC's own "revisited twice and
  settled" HOLDS the decision — and `staying`, which qualifies the reversal
  line, not the Postgres line), "notify customers by email" places the comms
  plan, "left out"/"discarded" name an omission, a refusal worded "turned
  down"/"vetoed"/"ruled out"/"passed on" is recorded, "I verified" grounds a
  live-state count, and a brief about "the shared secret"/"the credential"/
  "the HMAC check" engages with its item. Also honest now: `list_files` and
  `write_file` report real bytes (the sandbox said "bytes" and returned
  UTF-16 code units — identical on today's ASCII fixtures, a trap for the
  first fixture with an em-dash), the stale FORTY-SIX tool counts read
  FIFTY-FIVE (the number the sync test pins), and the timeout verdict names
  which clock fired — a model idle for minutes is not a model that was
  slow, and the inactivity clock now says so instead of letting a hung
  upstream read as the model's timeout. Verified: each fix carries a test
  that fails on the old pattern (the research repeat, the plan_doc
  paraphrase, the distiller refusal quartet, the inbox grounding and brief,
  the workbench repro, the judge inversion); full lib suite 1952 green.

- **Long agent turns finish instead of dying as "· interrupted."** Diagnosed
  from a fleet deploy: a knowledgebase-build turn that ran past ten minutes
  was killed by the API's own request timeout — a total 600s ceiling on the
  agent stream — while the agent's container was still working; the partial
  reply was dropped, the row landed as a bare error the chat rendered as
  "· interrupted", and nothing retried. (Compaction was never the culprit —
  it happens inside the container and simply made the turn long.) The turn
  now ends only on SILENCE: frames flowing — deltas, tool progress,
  keep-alives — keep it alive however long the work takes, with a tunable
  idle ceiling (`TALARIA_AGENT_IDLE_SECS`, default 10 min, floored at 1) and
  a bounded wait only for the stream to open. Liveness throughout is the
  last write, not the row's age: a new `streamed_at` stamps every flush, and
  the stale sweep, the sidebar's working flag, and the generating pulse all
  read it — an hour-long turn no longer reads as dead at ten minutes. A
  stream that does die is retried once, ON THE SAME ROW, after a short
  backoff: the row is resurrected to streaming and the turn re-driven with
  its own failed attempt excluded from the history, and the chat visibly
  picks it back up ("↻ stream dropped — picking the turn back up…") rather
  than sitting on an error. The error itself is now an explanation — the
  row's content says what happened ("the agent's stream went silent for
  600s mid-turn", the container's own failure reason when it refuses)
  instead of a bare interrupted mark, and it survives reloads and reaches
  the next turn's history. A turn that dies twice stays down for a person
  to look at; an agent's refusal (the failure frame) is never retried.

- **Agents can build out a knowledge space, not just append to it.**
  Reported live from a fleet deploy: an agent asked to put a space's intro
  and table of contents on the space page itself had no tool that could
  reach it — `create_kb_space` writes a landing body only at creation, and
  nothing after can edit it — so the content landed in a lookalike
  top-level doc, and with no move or delete the misfile couldn't be
  corrected, only duplicated. Three tools close the gap. `edit_kb_space`
  (name, description, icon, and the landing markdown; admission mirrors
  doc edits — authorship, an editor grant, or an elevated assistant on
  non-private material — and sharing fields stay human-only).
  `move_kb_doc` (nest under a same-space parent, lift to the space's top
  level, or reorder among siblings), under the same edit admission the
  doc's PUT already grants. And `delete_kb_doc`, deliberately narrower
  than the edit beside it: an edit is versioned and recoverable, a delete
  is not, so an agent deletes only docs it created and re-files anything
  else (`move_kb_doc` re-files without destroying). Doc deletes now land
  in the audit log whoever pulled the trigger. All three are mirrored
  through the fitness lockstep — catalog, sandbox backends, exercise
  tests — so the toolkit the next agent is trained against knows them.

- **API-CONVENTIONS tells the current truth.** The page still described a TS
  route tier the cutover deleted: the reference "frozen until #293" (the
  extractor landed — it is generated, and drift fails check), a TS twin of the
  agent-ticket predicate in `server/tasks.ts` (the file is gone; the predicate
  lives once, in `api/src/tasks.rs` — `agent_safe_patch` through `update_task`,
  the exported `agent_ticket_refusal` for every door that never reaches the
  patch), an invariants table naming rules that left with those routes
  (rewritten against the live script: swallowed query errors, flattened query
  results, dropped `listQuery` notices, hand-rolled popover engines, and the
  off-board / board-policy / fetch-door censuses), `.tsx` board filenames
  (`Kanban.svelte` and `BoardList.svelte` refuse a failed statuses read,
  `Gantt.svelte` marks it inline; the dead ci.yml note is gone), and an
  audit-task pointer that resolves nowhere. Every claim in the rewrite was
  re-verified against the code it names.

- **RELEASING.md says how main actually merges.** The branch table claimed PRs
  land "rebase-merged"; they land as merge commits, each merge subject carrying
  the PR number. Corrected so the doc a release follows matches the history it
  walks.

- **`deploy update` pulls the api package before it builds.** The app image
  never compiles the api — it bakes in the pre-built package named by the
  Dockerfile's `TALARIA_API_IMAGE` — and `up -d --build` resolves that FROM
  from the local daemon cache. So an update on a checkout host could git
  pull, rebuild, recreate, and report success while still serving an api
  binary from before the pull: new UI wrapped around an old api (bbills,
  2026-09-03 — one day-old package answered for three "already fixed"
  symptoms at once). `talaria deploy update` now `docker pull`s the package
  the Dockerfile names, between the git pull and the build — the ref is
  read from the ARG line itself, so the pull and the build can't disagree —
  and a failed pull dies instead of baking the stale digest on purpose.
  `deploy up` keeps its old leniency: that path is also the
  reboot-a-broken-box path, and must not suddenly demand the registry.
- **Reading a thread clears its bell rows.** Opening an agent chat, plan, or
  research discussion advanced the read cursor — the unread pill went away —
  but the notification rows pointing at the thread never learned, so the bell
  kept counting a thread you were visibly reading, and the only way to clear
  it was clicking the notification itself. The read route now sweeps: when
  the advanced cursor covers the thread's latest turn, the bell rows pointing
  at that thread (this reader's, at the href the writer filed) mark read
  alongside it. The href is computed by the same function the writer uses to
  file it, so the two ends cannot drift apart, and a read that stops short of
  the latest turn still leaves the bell alone — there is genuinely something
  left to read. The response carries how many rows cleared, so the client
  refetches the bell only when it actually changed; a failed sweep logs and
  still returns the cursor's ok (marking read must not 500 because the bell
  hiccupped).

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

- **Board column edits write again, and the DEK rotation sweep can run.**
  The fourth eruption of the port's typed-bind crash class, reported live:
  editing anything about a board column — label, color, category, and the
  agent-start flag that says where agents pick work up — 500'd on every
  attempt, first edit to the last. `update_status` resolves the row by key
  and reads it back `id::text`, then built the dynamic
  `update board_statuses set … where id = $N` and bound that string as TEXT
  against the uuid column: prepare resolves `uuid = text`, the operator does
  not exist, and the route answers 500 before anything moves. The cast rides
  the final bind now (`where id = $N::uuid`), matching the delete path that
  had it right all along. The same sweep found the identical shape in
  `secret_rotation`: every `CIPHER_TARGETS` pk is non-text (three uuid, one
  integer), so a DEK rotation would have died on its first row — the
  re-encrypt update now compares `"pk"::text = $N`, the same text the sweep
  reads them as. Proof in `api/tests/typed_binds.rs`'s house form: a
  live-DB test drives the real `update_status` (materialize, then the
  dynamic statement) twice with different fields and reads the rows back
  changed, plus a pinned unit test on the extracted `pk_where` builder; a
  sweep of every other `id/_id/_at = $N` site in `api/src` against
  `information_schema` found the rest text-typed or already cast.

- **Links that cross the app's boundary land where they claim.** Two href
  writers still emitted the pre-path-routing query shape after tabs and
  selections became path segments: a research run's completion notification
  pointed at `/research?r=<runId>` (which opens the bare research view with
  nothing selected — the quiet wrong answer) and a retrieved knowledge
  document pointed at `/knowledge/<docId>`, a one-segment form the Knowledge
  route parses as *space* with nothing selected. The writers now emit
  `/research/<runId>` and the by-id permalink `/knowledge?doc=<docId>`. The
  repair ships as the migration law's first exercise: two appended statements
  rewrite the stored rows (`notifications.href`,
  `briefings.source_href`) from `/research?r=` to `/research/` — idempotent
  and WHERE-guarded, applied at boot like every other one-shot. What a
  migration cannot reach — the href baked into emails already sent, and
  Qdrant payload hrefs until the next reindex regenerates them — is caught
  by a `?r=` redirect in the app shell, the same net `?tab=` already casts
  for the same reason.

- **The API reference is generated from the Rust sources again** — the #293
  extractor port. `gen-docs` now reads the router table
  (`api/src/routes/mod.rs`, cross-checking each registration's 405
  allow-string against its parsed method set) and the handler modules under
  `api/src/routes/**`: guards → the Auth column, `crate::body` member calls
  → the body tables, `json!` literals → Returns, `StatusCode` literals →
  Statuses, `// doc:` comment runs → notes. The three TS residents still
  serving `healthz`, `admin/update` and the app dispatch keep their TS-extracted
  rows, and a resident wins over any router twin (`/api/healthz` answers
  direct-binary checks in Rust; the documented surface is the proxy's). The
  frozen-at-cutover posture and its hand-maintenance are gone, and the
  `--check` drift tripwire in `bun run check` is live again. Regenerating
  against the freeze, every divergence fell into named classes: zod
  spellings → `crate::body` vocabulary, the honest 400s the TS extractor
  couldn't see (Rust spells body-parse refusals in the handler), error-shape
  Returns fixed to the real payloads, `/api/mcp/gw/{server}` re-sourced to
  its Rust module, and richer port response shapes (`{agent, registered}`,
  muse POST now an SSE stream).

### Fixed

- **GitHub App keys parse in the label GitHub actually ships** — App keys
  download as traditional `-----BEGIN RSA PRIVATE KEY-----` (SEC1) PEMs, and
  the signer read only PKCS#8, so every real key died at install time with
  `PKCS#8 ASN.1 error: PEM type label invalid` while the frozen fixture (a
  PKCS#8 key) kept the suite green. The parser now takes PKCS#8 first, then
  the SEC1 label — the same key either way, and a companion fixture pins the
  traditional label to the same signed bytes.

- **The port's text-vs-typed bind crashes: three statements that could never
  execute, found by auditing every query in the API.** sqlx declares each
  bind's wire type from the Rust value — a `String` bind crosses as TEXT —
  while the TS original's postgres.js let Postgres infer types per call.
  A text bind ASSIGNED into a typed column is coerced and harmless; a text
  bind COMPARED against one is a prepare-time `operator does not exist`
  that nothing catches at authoring time. Three carriers had been live-dead
  since the cutover: the 15-minute RAG sweep's four window queries all
  compared the ISO watermark against `timestamptz` columns, and
  `.unwrap_or_default()` ate the error — the sweep indexed nothing for
  weeks while its watermark advanced, silently burning every window (the
  mark needs a one-time reset on each instance so the repaired sweep
  re-covers them; indexing is content-hash idempotent); the daily brief's
  48h recency read 500'd on every still-current brief; the reindex run's
  artifacts page died whenever an artifact existed (`any($1)` of text ids
  against uuid). The casts now ride the binds (`$1::timestamptz`,
  `$1::uuid[]`), a live-DB suite (`api/tests/typed_binds.rs`, `#[ignore]`d)
  executes the three real carriers against the real schema, and reverting
  any one cast has been shown to fail its test with the exact fleet error.
  The same audit left the rest of the corpus clean: 910 of 968 statements
  proven legal by EXPLAIN against the live schema, the bind-cast and
  TS-parity sweeps converging on exactly these three, and the row-mapping
  sweep on one latent nullability fix — the inbox undo read mapped a
  nullable `outcome` column into a non-Option type, which now tolerates a
  completed-without-outcome row instead of 500ing the undo.
- **Devboxes are no longer API-dark on arrival.** The box compose's
  `environment: PATH:` mirrored the node base image's PATH — but a compose
  override *replaces*, and the devbox image prepends `~/.cargo/bin`: with
  cargo off PATH, `talaria dev`'s api sidecar skipped itself (non-fatally, by
  design) and every fresh box served a UI whose every `/api/*` call failed.
  The override now mirrors the devbox image's PATH — cargo restored, plus the
  sbin directories the abbreviated form had also dropped — so the sidecar
  raises the Rust api on in-box loopback as intended.

- **`list_boards` no longer 500s for agents — the port's one never-executed
  query.** The agent listing's `select distinct` sorted on `b.updated_at`,
  but the boards port had replaced the raw timestamps with epoch-ms
  expressions in the select list, and Postgres refuses a DISTINCT whose
  ORDER BY expression isn't selected — so the statement was rejected
  outright and every agent-authenticated boards call returned 500 from the
  day the module landed (the fleet's first smoke test found it; the
  user-session listing has no DISTINCT, which is why the UI never showed
  it). The epoch columns now leave as named columns and the sort uses the
  name. A live-DB suite (`api/tests/boards_store.rs`, `#[ignore]`d) now
  executes the listing against the real table so an illegal shape can never
  land again.
- **A personal research run's report is readable by the agent that ran it.**
  The toolkit's own instructions tell the agent to read the report back with
  `get_document`, but a personal run saved its report private with
  owner-only grants — the agent got 403 on the artifact it had just been
  asked to produce. The run's agent now lands a viewer grant alongside the
  members shared on the run, matching what the brain's index already assumes
  ("them + their assistant"). The pre-port TS had the same gap, so nothing
  changes for anyone else.
- **The api no longer times requests out.** The Rust router carried a 30s
  `TimeoutLayer` (503 on expiry) that the TS server it replaced never had —
  a port-scaffolding pick that broke every legitimately slow surface it
  touched: long agent turns and tool-call chains, Google syncs, heavy reads.
  Routes it broke had been getting carved out of it one at a time into a
  second "streaming" stack until the split was mostly scar tissue. The layer
  is gone and the two stacks are merged back into one: requests fail on
  errors — panics, refused guards, upstream call budgets — never on a clock.
- **`check-docs` no longer reads code spans as links.** A generated schema
  cell like `` `uuid[](50)` `` is markdown's `[text](target)` shape wearing
  backticks; the link checker now masks inline code spans before matching,
  so generated tables don't trip the dead-link rule.

### Removed

- **The dead TS engines go too — `ui/src/server` is down to its live tier.**
  The cutover deleted the routes; this strip deletes what they left behind:
  ~106 files (54 engines + their tests) removed by a reachability audit from
  the live seeds (server-entry, `app.ts`, the four residents, the SDK, the
  non-server `ui/src` tree, the CLI and mcp packages) — the boards/tasks/
  statuses/approvals/notifications engines, the scheduler and its lease,
  inbox-focus, research/outreach/work-dispatch/briefing, the Google engines,
  fleet render/reconcile/preflight/brain/crons, storage/uploads, retrieval,
  channels/comms, daily-brief, muse route halves, and more. Two near-misses
  the audit caught before they shipped: `app.ts` itself (the SPA's SSR entry —
  vite dev-middleware, the server build, and boot's `migrate` hook all reach
  through it) was restored, and `scripts/gen-github-jwt-vectors.mjs` (wired
  into `api:vectors`) imported the deleted signer, so it was deleted and the
  fixture it generated is now frozen ground truth for
  `api/tests/github_jwt.rs`. The TS updater's scheduled check went with the
  scheduler: the job never registers, the manual check/apply halves and the
  admin panel stay, and the hold is documented at both ends (the Rust api
  cannot rebuild and restart its own binary — see `api/src/jobs.rs`). Two
  invariant rules retired with the surfaces they watched (the
  hand-written-harness census and the toolbox anchor re-pointed at the Rust
  fitness toolbox); a new cross-language pin test reads
  `OFF_BOARD_STATUSES` out of `api/src/statuses.rs` and fails if the TS
  client's copy ever drifts (fail-closed: an unparseable literal parses to
  an empty list and fails). The surviving ~54-file tier is everything the
  residents, the SDK, and the SPA shell actually import — plus the comment
  and doc citations across the tree re-pointed at their Rust twins.

### Changed

- **Comments and docs live in the present.** The port-era narration is gone
  from the tree: ~470 Rust/TS files' comments no longer cite their TS
  originals, batch/wave provenance, cross-references to deleted files, or a
  parity battery that finished — each now states its own contract directly
  (wire shapes, key order, invariants, ordering and parse-order facts,
  security reasoning, test pins all kept). Stale port TODOs whose conditions
  long since landed were deleted rather than rewritten (registry's
  still-to-cross ids, define's DEFERRED block, artifacts' batch-5 notes,
  render's ensure_mcp_service). What remains TS-named is deliberately so:
  the four permanent residents' rule-10 facts, the live TS twin contracts
  (`ui/src/server/tasks.ts`'s agent-authority predicate, `task-const.ts`,
  the secretbox cross-language cipher), and `mcp/src/index.ts` sync pins.
  Every changed line verified comment-only by a diff proof over the whole
  tree. The docset follows: HARNESSES.md now points at the Rust harness
  layer with the TS twin framed as the app-author plane; API-CONVENTIONS.md
  makes the Rust crate the runtime of record with the TS dialect governing
  the residents and app servers, and names both homes of the HITL
  predicate; ARCHITECTURE, AGENT-NETWORKING and DEVELOPERS read current;
  CONTAINER.md's fleet-manifest link points at `api/src/fleet/render.rs`
  (was the dead TS renderer); and the port's own narrative moved to
  `docs/history/rust-port.md` when RUST-MIGRATION.md split into living
  rules + frozen record. Old references live exactly where they belong
  now: this changelog and `docs/history/`.

- **api/src is organized one dir per subsystem.** The flat layout — 217 route
  modules and 149 top-level engines in two undifferentiated piles — is gone.
  `api/src/routes/` now carries 23 group dirs mirroring the `docs/api` groups
  (account, boards, comms, fleet, knowledge, …), the map inverted from the
  reference's own Source links; the router table's handler paths are
  group-qualified, so the table names the system. The engine families fold
  into dirs with their prefixes dropped (`crate::fleet_render` →
  `crate::fleet::render` — the shape `fitness/` and `runs/` were born with):
  `fleet/`, `google/` (gmail included), `mcp/`, `model/`, `inbox_focus/`,
  `kb/`, `workbench/`. Pure moves and path rewrites — no behavior change;
  module names stay (docs/api links re-pointed, provenance headers intact),
  `module_inception` is allowed at the crate level for the flagship-module
  shape (`routes/agents/agents.rs`), and the top level keeps its true singles
  (101 files, down from 149).
- **First spin-up assumes the Rust api — the hop is the default, not an
  opt-in.** `TALARIA_RUST_API_URL` unset now hops to the loopback default
  `http://127.0.0.1:5274` (the same port `talaria dev` and the production
  server-entry bind their api to), so a fresh checkout proxies the moment an
  api is listening — the coexistence posture of unset-forwards-nothing died
  with the TS routes it was protecting: post-cutover it served nothing but
  404s. The literal `off` stands the hop down for the one posture that wants
  no api behind the process (tests); every other posture — a set URL or the
  default — keeps the no-fallback rule, so an api that is down answers 502.
  `talaria setup` writes the URL into the generated `ui/.env`, `talaria dev`
  lifts it into the vite child unless the shell or `ui/.env` names one (an
  explicit value is never overwritten), healthz probes the effective URL
  (the default included; `off` skips), and server-entry honors `off` without
  probing or spawning. The docs cross with the code: RUST-MIGRATION's
  coexistence narrative is past tense end to end, and "Where it stands"
  records the cutover as landed.
- **The cutover lands — the TS API is deleted; the Rust api is the only
  runtime.** The `cutover/remove-ts-api` branch takes the coexistence
  posture the port left and finishes it, in waves: the proxy flip deletes
  every migrated TS route file and their server halves (SvelteKit keeps the
  SPA and the four permanent TS residents — healthz, `admin/update`, the
  rule-10 `/api/apps/` dispatch, the app-MCP gateway); the server prune and
  the scheduler crossing dissolve the flip predicate (`TALARIA_SCHEDULER`
  shrinks to a kill switch: unset arms, `off` stands down, `rust` is the
  retired handoff value that reads as unset — the api is the only thing left
  to arm); `server-entry.js` owns the api binary in production, adopting an
  instance already on the port or spawning one (`TALARIA_API_BIN`), wiring
  the proxy hop itself, and dying with its child so the supervisor restarts
  the pair; and the api ships **pre-built** — `api/package.Dockerfile`
  compiles the musl-static binary in CI only, published as
  `ghcr.io/outcrop-labs/talaria-api` (tag `main` on every api-touching
  push — the same ref Dokploy's checkout builds build — plus immutable
  `sha-<sha12>` provenance tags and channel tags mirroring the app image's),
  and the app image consumes it with `COPY --from`, so no GitHub runner,
  operator machine, or customer VM ever compiles Rust to build the app
  image. healthz grew the third check (`rustApi`) and `talaria dev` now
  spawns the api by default (`TALARIA_API=off` opts out). The generated API
  reference froze at the cutover: its Source links point at the Rust modules
  (`api/src/routes/**`), `gen-docs` skips `docs/api/**` until its extractor is
  ported to the Rust router table (#293), and the CLI reference generates as
  before.
- **The port merges — the Rust API is the backend of record.** PR #290
  landed on main (2026-09-01): 216 of 219 TS route files serve from the Rust
  crate in any proxied environment, the parity battery closed at 370 route
  pairs byte-diffed against the TS oracle on the shared dev DB (every pair
  either byte-identical or a recorded divergence — that record is the
  contract, `docs/RUST-MIGRATION.md`), and the scheduler flip is armed in
  dev (`TALARIA_SCHEDULER=rust` — the whole job table from `api/src/jobs.rs`,
  TS's `startScheduler` stood down on the same value). CI gained the `api`
  job: rustfmt, clippy `-D warnings`, and the crate's tests on the pinned
  1.97.1 toolchain (`bun run api:check` runs the same gates locally). The
  three permanent residents stay TS (`admin/update`, the rule-10 app
  dispatch, and `healthz`); cutover — deleting the TS API behind the proxy —
  is the one remaining step.
- **The tail crosses — the last 27 TS route files proxy to Rust; coverage
  closes at 216 of 219.** The remaining singles and the fleet defs detail
  trio crossed as one sweep: the alert feed, home cards, global search (the
  hand-rolled SearXNG federation over a local FTS pass), the Muse, the
  inference ledger read, the join-code claim, the instance card at
  `well-known/talaria-instance`, `me/assistant` (the agent-start trio), the
  gap ledger (list + `{id}` resolve), the template library (list/create +
  `{id}` patch/delete), the skill registry (list + `{owner}/{name}` CRUD),
  the plans doc/members pair (with the plan-engine trio
  `ensure_plan_doc`/`plan_tier`/`sync_plan_doc`), the agent plane under one
  character prefix (`agent/gap`, `agent/problem`, `agent/message-user` — the
  refusal paths 403 on a fake taskId before any ticket/board write — plus
  `agent-media` both files and the role templates), the image describer
  `/api/vision/describe`, the remembered-facts plane `memory/{id}`, and the
  hire editor's versioned trio (`defs/{id}`, `/edit`, `/versions` —
  `revertTo` of the current version writes nothing and answers
  `created:false`; a fake endpoint 400s before any write). With the sweep
  the proxy's residents are exactly three, all permanent: `admin/update`
  (it rebuilds `ui/dist` and restarts the bun process it runs in —
  redesign at cutover), the rule-10 app dispatch, and `healthz` (served by
  both, never proxied). The byte-diff harness — 85 rows, 0 fail, 3
  documented world-dependent divergences (alerts/home read rows their own
  loops wrote; search federates live engines) — found four port bugs, all
  fixed: zod's enum message for template kinds (`Invalid option: expected
  one of "ticket"|"plan"` via `enum_member`), the skills file order (node's
  readdir SORTS where tokio's `read_dir` returns raw getdents order — both
  listing sites now sort), `me/assistant` PATCH's render propagation
  (`renderFleet()` has no catch in `updatePersonalAgent`, so a render
  failure 400s the whole PATCH with the version row already written and
  standing; only docker restart/up stays best-effort), and the bare
  secretbox message (`ensureAgentApiKey` propagates `open(existing.keyEnc)`
  unwrapped — no wrapper sentence).
- **The fleet plane speaks Rust — every remaining `/api/fleet` route
  plus the agents register/heartbeat pair.** Sixteen route files
  crossed as one unit: the ops overview (`/api/fleet` — heartbeats
  seeded from defs, container reality filling the liveness gap so the
  online count can't disagree with the roster dots), containers, the
  defs listing, crons fleet-wide and per-agent with the per-job trio
  (delete/edit/pause-resume-run), the per-agent secrets vault door
  (PUT set, DELETE with its query-param fallback when the body won't
  parse), the seven-verb control union (up/stop/restart/roll/retire/
  unretire/delete with the audit-before-act discipline and the catch
  arm that flattens every interior failure into the generic 500),
  render, endpoints CRUD (create/patch/delete/available), federate,
  and reconcile — plus `/api/agents/register` and
  `/api/agents/{id}/heartbeat`, the shared-key fleet plane, now
  reading `TALARIA_AGENT_KEY` in the Rust process too. Two wire rules
  the byte-diff pinned down and now hold crate-wide: pg `numeric`
  columns ride the TS wire as STRINGS (postgres.js passes them through
  unparsed — a stored 3 is `"3"` — so the endpoints listing selects
  `::text`), and timestamptz-to-ms TRUNCATES where a pg cast would
  round (JS `Date` drops the fractional millisecond — the overview's
  last-used is `trunc(extract(epoch …)·1000)::bigint`). The heartbeat's
  non-uuid path 500s on both sides for different reasons that meet in
  the same envelope (postgres.js's throw vs the `$1::uuid` cast) —
  byte-verified. Verified by a 64-case byte-diff against TS (every GET
  × admin/member/anon gates, the zod 400 tables, the render POST, the
  endpoints CRUD round-trip on per-side fixtures, `available` against
  an unreachable baseUrl, fake-id 404s, and the register→heartbeat
  round-trip with the real org key): 64 byte-identical, 0 normalized,
  0 unexplained; lifecycle verbs on real agents stayed SKIP-listed
  (container writes on the shared dev box). One divergence recorded:
  a permissions-read failure at the crons/secrets gate answers
  fail-closed 403 where TS would 500. The defs detail trio
  (`defs/$id`, `/edit`, `/versions`) stays TS for the next slice.
- **The fitness plane speaks Rust — the probe/eval/adversarial battery,
  its run engine, its archive, and the whole verdict surface.**
  `/api/admin/model-fitness` crossed as one route over six engine
  slices: the matrix (every gateway model against every harness),
  capabilities, health, value (the money view — workload shares, the
  USD-per-ready-run arithmetic), detail with its live-run block and
  stale-run sweeping, estimate (fixture arithmetic over the last
  archived report), and transcripts, plus the action union —
  start/stop/clear/forget with the arming ladder (role checks, the
  adversary-may-not-be-the-candidate refusal, concurrency 1..8).
  Three port principles came out of it and now hold crate-wide. Stored
  jsonb never rides a typed struct to the wire: Postgres keeps object
  keys in its own canonical order and TS passes the parsed blob
  through untouched, so every stored-object passthrough (the detail
  `record`, run rows, both views' `index`) rides the raw
  `serde_json::Value` while typed parses serve decisions only.
  `JSON.stringify` prints whole floats as integers (`1`, not `1.0`),
  so responses go through a `js_numberify` walk. And serde_json's
  default float parse is best-effort — it can land one ULP off the
  parsed digits, which a 456KB otherwise byte-identical body proved on
  a stored `0.9090909090909091` — so the `float_roundtrip` feature is
  on for every float the API parses. Verified by a 27-case byte-diff
  against TS (all seven views × archived/unarchived/missing models,
  the estimate query-param folds, the anon/member gates, and eleven
  POST bodies): 21 byte-identical, 6 exactly the app-harness boundary
  (app-shipped harnesses are TS-only by rule 10, so their counts and
  arithmetic differ on any install with an enabled app — recorded
  under divergences), 0 unexplained. The start body's zod union was
  probed against the verbatim TS schema: any parse failure collapses
  to the union's `"Invalid input"`, and only bound failures surface
  the arm's own sentence, in field order. The diff also found a TS
  bug, not a port one: `clear` 500s on TS for every model
  (`setSetting(key, null)` writes SQL NULL into a NOT NULL column) —
  Rust keeps the working behavior and the bug is flagged for the TS
  side.

- **The admin tail speaks Rust — all fifteen admin routes, the secrets
  inventory, and the engine halves behind them.** Settings, users, the
  secrets inventory, encryption, domains, invites, email, search, judge,
  outreach, guardrails, platform-agents, storage, apps, and the
  workspace-secrets admin console crossed together, carrying their engines:
  `secret_health` (presence and provenance over every store that holds a
  sealed value — never the value), the rotation engine (unit-verified, not
  fired on a shared dev box), invites with their email half, the email
  config and test send, search reachability, the apps discovery/enable
  plane, and the nine-action workspace-secrets union. The slice's real work
  was fidelity to zod's own rejections: every body was re-probed against the
  TS oracle and rewritten onto the grown `body.rs` helper set (the enum's
  `Invalid option: expected one of` with no received clause, `expected int,
  received number`, the literal's `expected true`, custom grammar sentences,
  and the tri-state `nullish_member` that tells an absent patch key from a
  null one). Verified by a 60-case byte-diff; the diff caught a silent-200
  bug (an invalid role enum value was dropped, not refused), a doubled
  backslash in SQL that 500'd the encryption GET, and the guard findings'
  float4 confidence decoding into an empty list. `talaria dev` now lifts
  `SEARXNG_URL` for the Rust process so the search panel's env flag agrees
  with its own probe.

- **The mcp family speaks Rust — the org registry, the per-user connect
  plane, the OAuth pair, and the agent gateway that relays (or refuses)
  every tool call.** Eleven route files crossed: the per-agent roster
  (members get server names only), the registry pair with its
  create-and-sniff (a 401 challenge with resource metadata flips OAuth on),
  me/mcp connect/disconnect with credentials sealed at rest, the
  fleet-defs versioned MCP hook, the OAuth start/callback ladder, the test
  probe, the icon fallback, the library shelf, and `/api/mcp/gw/{server}`
  POST+GET — registered on the streaming router, because a legitimate
  tools/call holds up to 120s and the GET is a live SSE stream. The
  gateway keeps the family's security lines: identity resolved from the
  credential (never the claimed name), the caller's OAuth token spent only
  through the effective-server resolver, secret handles resolved on the way
  OUT and never on results, the per-assignment tools gate, in-process
  dispatch for the workbench, and SSE tools/list filtering that
  re-serializes only `data:` frames. App servers stay TS by the port's
  rule 10 — a STAY_TS hole in the coexistence proxy carves
  `/api/mcp/gw/app-*` back out of the family's prefix. Verified by a
  75-case byte-diff against TS with every network touch pinned to a
  deterministic answer (loopback refused, `.invalid` DNS, and the real
  loopback services: the builtin toolkit relay and the workbench's
  in-process dispatch). The diff caught five port bugs the unit tests
  could not: unaliased computed columns in the registry's row read (every
  full-row read 500'd — sqlx's derived FromRow matches by output column
  name), `created_by` bound with a uuid cast against a text column (every
  created agent version 500'd), zod's `Invalid URL` sentence casing, the
  icon route's bare 404 (no body, no content-type), and org discovery's
  transport errors, which must read undici's shapes (`fetch failed`) --
  reqwest's messages never reach a TS caller.

- **The workbench family speaks Rust — all seven routes, github's REST
  half, and the MCP dispatcher the fleet's agents speak.** The profile
  registry (member reads mask env values to `•••`, the image/mounts
  infrastructure fields stay admin-only), the per-repo git flow, the org
  GitHub connection's read plane, the harness registry (declarative
  custom definitions stored in probed zod shape order, builtins
  undeletable), the human side of workbench jobs (the ticket strip, the
  approve/reject gate through board editors, merge-to-testing through
  the engine with both failure flavors folded into the TS `.catch()`'s
  one 400), the repo-creation approval queue, and the per-agent repo
  grants. Verified by a 123-case byte-diff against TS — the largest
  family harness yet: gate tables for anon/member/admin, the full zod
  400 tables for all seven bodies, restore-safe mutation cycles, twelve
  SQL-seeded jobs with fixed ids and timestamps (approve/reject ±note,
  the re-approve's `job is started`, the null-task 403, both merge
  refusals on the unconfigured connection), and the repo-request reject
  ladder. The diff caught three things the unit tests could not: the
  THROWN-500 shape — defineApi has no catch, so a TS handler that throws
  answers SvelteKit's text/plain `Internal Server Error`, and all 498
  port sites mirroring a TS throw were swept to that shape port-wide;
  the profile autoAttach wire, which is the stored column's jsonb
  canonical order, not the parser's shape order; and the harness list's
  custom order, which no runtime orders at all (no ORDER BY) — the
  harness pins the builtin prefix and compares the customs as a set.

- **The integrations/google family speaks Rust — the whole OAuth pair, the
  org plane, the user surfaces, the approval queue, and the agent plane,
  twenty-one routes under one prefix.** Every door reads the same
  connection rows, so the family crossed whole: the personal
  connect+callback ladder (five deterministic refusal rungs), the org
  plane (its own connect pair, status/targets/disconnect, provisioning,
  live health probes), the per-surface user reads with their query-param
  folds, the three user writes that draft through the approval queue
  rather than send, the pending decide, and the agent surfaces —
  `refuse_legacy` first, the `x-agent-name` must-equal-model check, then
  a resolution rule where a personal assistant acts strictly as its
  owner's connection and a fleet agent as the shared org account. The
  gmail engine crossed as its own module; verified by a 69-case byte-diff
  against TS on the same sessions plus two agent keys, including live
  reads against the real org connection (calendar, drive, gmail, labels,
  one full message) and the one designed round-trip: an agent drafts a
  send, a member's verdict is refused, the admin's reject retires it —
  nothing leaves the building. The diff caught two port bugs: the
  targets wire's key order (embed the typed struct — its declaration
  order is the wire order) and the provisioning filter that seats only
  org agents in the send-as table. Also the port's first silent-trait
  lesson: a non-`Send` form serializer held across an await in an engine
  fn fails every handler that awaits it with a bodyless E0277 — scoped
  and finished before the await now.

- **The secrets family speaks Rust — the working vault crosses whole, and
  the port's first cross-runtime cipher proof lands with it.** Six routes
  under one proxy prefix: the collection (list/create/file/move/delete),
  the folders pair (create/rename/delete/share in one union body), the
  reveal (with its refusal ladder and the no-store header triad), the
  share pair for people and agents, the one-shot relay, and the git
  credential helper — the agent's door to `git.example.com`, which spends
  a workspace credential or a minted relay and falls through to the GitHub
  app token. The engine crossed with them: the vault's seal/open, the
  credential selection that makes a single-entry doc a token
  (`x-access-token`), the relay burn, and the folders' reveal-vs-spend
  asymmetry. Verified by a 127-case byte-diff against TS on the same
  sessions, with the interop proof as its centerpiece: both runtimes share
  one database, so the harness seals the same literal through each
  runtime and reveals it through both — the same value back every time,
  which is what the secretbox port was for. GitHub app JWTs gained a
  cross-language vector file (a committed fixture key, three clock
  instants, byte-equality both ways). The diff caught three port bugs:
  the listing's `distinct`/`order by` (Postgres requires the ordered
  column in the select list — every listing 500'd), the move route's
  admin thread (owner-only even for admins, as TS passes no isAdmin),
  and the union body sentence (zod unions flatten every failure — a
  non-object body included — to plain "Invalid input").

- **The brief family speaks Rust — the assistant's morning document crosses
  with its reader.** Five routes under one proxy prefix: the document read
  (sweep-if-due, then the fold), the line verdict (check/dismiss/restore
  with its three 404 sentences), the read cursor (monotonic — it only ever
  advances), the delegation pair (grant/revoke the reply-without-asking
  privilege, where granting permission for an already-written reply means
  sending it), and the draft verdict — the one route in the family that
  causes something to leave, so it is the one that refuses on staleness
  (409 "They have said something since this was written…" rather than a
  400). The engine's reader plane crossed with them: the absent literal
  `{absent, nextAt, agent}` across all three kinds, the fold as typed
  structs in TS declaration order (a resolved line sinks but is never
  dropped), and the delegation state machine. Verified by a 39-case
  byte-diff against TS on the same sessions — the document read literally
  (all three `tz` variants), every validation shape, the check-off
  idempotence and the restore ladder, the grant→release(sent:1)→revoke
  cycle, the foreign-channel 403, and the reply ladder. The brief is one
  document per person per day, so the harness warms the shared document
  first and then works per-side probe rows and seeded resolved pairs. The
  diff caught four decode bugs on its first run — the same two families as
  before (uuid columns decoded as `Option<String>` need `::text`; INT4 seq
  columns decoded as i64 need `::int8`). The `pending` absent kind is
  unreachable at the harness's hours and is pinned at the engine level
  only.

- **The inbox.focus family speaks Rust — the assistant's attention plane
  crosses whole.** The focus queue, badge summary, viewed/snooze state,
  card actions (with their confirmation tokens), the SSE command stream
  that runs an instruction through the focus assistant, and the segmented
  conversation picker serve from Rust under one proxy prefix — the
  family's per-user lock spans the command stream and the state route, so
  the prefix carries all eight paths. The engine crossed with them: the
  four-source queue builder (approvals, tasks, comms, notifications) with
  buckets and briefs, the action policy, the timeline, and the approvals
  read; the assistant itself is a harness def and became the streaming
  seam's first caller (`dispatch_transport` — gateway or fleet persona by
  kind — plus `StreamOptions`). One recorded divergence, documented at the
  route: on a client disconnect TS leaves the assistant row `streaming`
  while the port finishes and persists the reply (the chat family's tee
  philosophy). Verified by a 31-case byte-diff against TS on the same
  sessions — the reads across anon/member/admin, the validation shapes,
  and the writes through per-side seeded rows (mark_read consumes its row,
  so each runtime acts on its own notification), plus three SSE command
  streams byte-identical via a manifest-absent model's canned reply. The
  diff caught six port bugs, worst among them a member leak in the
  approvals source: TS splices the admin arm out of the WHERE clause for
  members entirely, and the port's first shape (`or is_org = $2`) matched
  every other person's personal actions. Also fixed: SSE frames carrying
  `conversationId` snake_case (an enum's `rename_all` camelCases only the
  variant tags — `rename_all_fields` does the fields; pinned by a test
  because the compiler cannot see the difference), two Postgres
  param-type 500s and a UUID-decode 500 in the picker, the summary's key
  order, and the command stream's header order.

- **The conversations family speaks Rust — the durable chat crosses.**
  `/api/chat` (whole-path, the family's only EXACT proxy entry), the
  `/api/conversations` list and `{id}` detail/rename, and `/api/dms` serve
  from Rust. The engine crossed as `chat_persist`: the gateway stream is
  teed through a bounded channel whose receiver is the client's SSE body,
  while the drain that persists history runs detached — an in-progress reply
  survives a client hang-up. The persist half carries the throttle (400ms,
  first event immediate), the token ledger (real usage or a char estimate),
  the confab guard awaited AHEAD of the plan-brain index and the @mention
  mail (strict mode redacts before either takes its copy), the Titler's
  first-exchange rename, and the queued-message continuation chain — a
  message that landed mid-stream starts the next turn, honoring the effort
  stamped on its own row against the model that will actually run it. The
  proxy's response-header allow-list gained `x-conversation-id` and
  `x-message-id`: the SPA learns a new conversation's id from the header.
  Verified by a 69-case byte-diff against TS (reads across chat/plan/
  research access, the dms table, the zod 400 table, a write cycle through
  both runtimes including a queued 202 and a collaborator's rename, and
  the SSE streams byte-identical via a manifest-absent model's canned
  reply), plus a live-agent turn proving stream → persist → ledger with
  real token counts end to end. The diff caught three port bugs: the
  detail select's one nullable column (`guard` — null on the wire, not
  `[]`), the DM's foreign-key 400 carrying sqlx's decorated message where
  TS answers the bare Postgres sentence, and the SSE header order
  (`cache-control` belongs above `content-type`).

- **The channels family speaks Rust — the comms plane crosses whole**: all
  ten `/api/channels*` route files (list/create, `{id}` detail/rename/
  archive/hard-delete, the agents pair, conclude, the SSE events stream, the
  members pair, messages GET/POST, message PATCH/DELETE, reactions, read)
  serve from Rust under one proxy prefix, which subsumes the plan-draft
  mount's old path-shape entry. The engine crossed with them (the message
  insert/list/edit/delete, members, agents, the reaction toggle, read
  cursors, the relay-summary conclude, the hard-delete's detached qdrant
  purge) plus the reply plane it drags along — @mention notifications, the
  DM peer notify, and `trigger_agent_replies` with its streamed replies.
  The POST's echo carries a different wire shape than the list (the insert's
  RETURNING selects no guard/editedAt — the keys are absent, not null), so
  the port serializes the page shape and projects the insert shape from it;
  the agent GET strips `guard` outright, keeping findings — flagged
  content's verbatim excerpt — out of model contexts. Verified by a
  102-case byte-diff against TS on the same sessions (reads across the
  channel kinds, a restore-safe write cycle through both runtimes, the 405
  `allow` tables, the zod 400 table with its parse-order probes), which
  caught three port bugs the unit tests hadn't — a mis-numbered bind that
  500'd every message edit, the rename's min-length bound, and a fractional
  `since` that floored where TS fails.

- **Version history speaks Rust — and the port gained a byte-identical YAML
  emitter.** `/api/history` serves both stores (`internal_versions` snapshots
  and `agent_versions`) through the live items' own read models applied
  backwards, every miss and every error reading 403 — fail-closed, or
  history is a permission bypass. `kind=config` serves `stringifyYaml`
  bytes, so the slice dragged `api/src/yaml_string.rs` across: a source port
  of the npm `yaml` package's stringify pinned by 145 fixtures the REAL
  package generates.
- **Artifacts, folders, links, uploads, and the Drive export speak Rust.**
  Eleven routes over three proxy prefixes (`/api/artifacts` does NOT
  prefix-match `/api/artifact-folders` — character 13, `s` vs `-`), with the
  write half of the engine: version snapshots, the official→KB mirror
  through the real qdrant/embed deps, the first-publish slug mint, the
  streamed multipart capped BEFORE it buffers (a declared over-cap
  content-length answers 413 unread), and the Drive export proven live
  against production Google from both runtimes. The diff's finds:
  `artifact_links.target_id` is TEXT (three `::uuid` casts refused to match),
  and the PUT's title tri-state — absent means don't-touch, never set-null.
- **The knowledgebase speaks Rust — all twelve kb.* route files crossed
  whole** (spaces, docs, comments, backlinks, move, presence, search, public
  slugs) with the ACL engine they share. The 107-case byte-diff found four
  port bugs the unit tests hadn't: uuid binds where the column wanted none,
  the search hit's raw-row wire shape, the float4 rank decode (f32→f64
  promotion prints `…584` where JS's parseFloat prints `…522` — the faithful
  path is `::text` then parse), and PG's bare sentence where sqlx's Display
  wraps it.
- **The retrieval plane speaks Rust — the reindex pair, the admin console,
  and the rag family.** rag-backfill/rag-reindex crossed with real deps (the
  artifact routing and the health probe beside them), then `admin.rag`
  (status + run projections, the reranker config, space↔brain bindings, the
  enqueue kicks), then the `/api/rag` family whole — the collection registry
  the MCP `search_knowledge` tool resolves principals against, the member's
  blanked binding matrix, and the search itself. The upgrade-status cache
  invalidates from inside the process that rebuilds it.
- **The runs engine and the scheduler cross — the port's flip.** The
  durable-run core (Redis leases with CAS renew/release, every `runs` write
  a CAS on `(id, lease_owner, state)`, the step-budget lease TTL, the
  `awaiting` guard that parks rather than fails), the six run kinds, the
  boards family with its dispatch re-entry, the SSE streams, and the whole
  registered job table — comms-decay, outreach-sweep, price-refresh,
  daily-digest, approval-escalation, notification-mail, run-reclaim,
  daily-brief, mcp-library-refresh, update-check, with `maybeRewriteBlurbs`
  a REAL job at last (dark in every proxied environment until now — its
  only TS trigger was the `/api/models` handler the proxy shadows).
  `TALARIA_SCHEDULER=rust` hands the whole schedule over in one slice —
  stop-TS, arm-Rust, never both — and the arm refuses to fire until the
  census's run kinds all have definitions. Schema ownership handed to sqlx
  with it: the same `schema_migrations` discipline (per-statement sha256
  bookkeeping, the same advisory lock, growth-only re-arm) now lives in
  `api/src/db.rs`, and the TS array is frozen. `update-check`'s apply half
  is a deliberate hold — it rebuilds `ui/dist` and restarts the bun process
  it runs in, choreography that cannot restart the Rust binary — so its
  auto half returns at cutover.
- **The port's end state corrected: Rust is the backend, not the UI host.**
  The roadmap's cutover batch said Rust would serve the SPA and `bun run
  start` would become the Rust binary — that was never the ask. SvelteKit
  keeps serving the SPA, its pages, and routing (TanStack stays the client
  layer, untouched). Cutover now deletes the TS API — `ui/src/routes/api`
  and the backend of `ui/src/server` — while the SvelteKit server remains
  the one public origin, handing every `/api/*` request to Rust on loopback
  (same-origin cookies, so the proxy hop graduates from scaffolding to the
  permanent architecture). The musl build stage stays: it builds the Rust
  backend process the prod image runs alongside the frontend server.

- **The model-identity plane speaks Rust, closing batch 3**: `/api/models`
  (the picker catalog — gateway rows carrying label and blurb from
  OpenRouter's public catalog, plus the persona-resolved `effective` model),
  `/api/models/efforts` (the composer's effort ladder for a persona or
  catalog id, with the agent-configured default held against the levels the
  model actually publishes), and `/api/admin/model-roles` (the Model Roles
  panel — assignments, fitness issues, per-role effort preference, GET and
  PUT). The three stand on one harness persona engine, so they crossed as
  one slice and the engine came with them: the persona index's two-pass
  claim (base ids first; alias tiers second with NO main-configured gate; no
  main, no keys; every pool carries the fallbacks the runner can land on),
  the effort ladder (provider catalog plus the admin declaration, which
  replaces and never merges, with the once-only backfill for catalogs stored
  before effort extraction existed), the model-info matcher (6h
  stale-on-failure cache, `model_blurbs` override whose read error
  propagates exactly as TS's `Promise.all` does), and the resolution chains
  behind `effective` (pin→role→utility→env→first-routable; the member
  allowlist gates every link except pin and env). Verified by a 37-case
  byte-diff against TS on the same sessions — the full zod 400 table behind
  the PUT, restore-safe write cycles run through both runtimes with the
  audit rows mirroring, the 405s with their `allow` headers, a duplicate
  `?model=` (URLSearchParams.get is first-wins), and the persona-configured
  default read through a live config mutation after both runtimes' persona
  caches expired. Three cases differ in array order alone — the recorded
  localeCompare divergence, seen for the first time now that an endpoint
  name carries a capital letter (TS collates `Z.ai/glm-5.3` last, byte
  order puts it first). Two divergences recorded: the org-voice blurb
  sweep's only TS trigger was the `/api/models` handler the proxy now
  shadows, so it is dark in proxied environments until it ports as a real
  scheduler job (batch 4) — Rust reads the rows, never writes them — and
  the blurb clamp cuts at a char boundary where TS's UTF-16 `slice` can
  split a surrogate pair.

- **The profile speaks Rust**: `/api/me` — the three preference reads
  (preferred model, platform-default reasoning effort, IANA zone) and the
  profile PUT: display name (users row AND the live session, via the
  KEEPTTL patch — the SPA's corner never waits for a re-login), the member
  model gate (enforced at the route, not just hidden in the picker), and the
  zone check. The zone validator is a probed contract, not a regex: TS asks
  Intl to resolve the name, and node's and bun's ICUs were probed on a
  100+-spelling table where they agreed on every row — IANA names match
  CASE-INSENSITIVELY ("utc", "Etc/gmt+5" resolve), offset forms `±HH`,
  `±HH:MM`, `±HHMM` resolve up to hour 23 / minute 59, and `Factory` is
  refused. The port mirrors that grammar (chrono-tz's tzdb + a hand-rolled
  offset parser) and pins the same table in its tests. The route migrates
  by EXACT pathname — the me.* siblings (mcp, assistant, events) are their
  own planes that move whole with their batches. Byte-diffed against TS on
  the same DB: the full zod 400 table, the trim corners (a spaces-only name
  is legal to zod and stores ""; a padded zone trims before it validates),
  the application order (a name in one PUT lands before a refused
  model/zone answers the same PUT — TS applies fields in sequence and the
  port invents no transaction), the member gate's exact 403 sentence with no
  write behind it, and the 405s.

- **Notifications speak Rust**: `/api/notifications` — the bell's one read
  (inbox, unread count, routing prefs, the digest answer, the instance email
  switch, and whether this user may flip it), mark-read by ids or all, the
  per-class routing patch, and the admin delivery switch. The mail side of
  the prefs (sendGatedMail, the bounded outbox, the drain) is scheduler
  plane and stays TS until batch 5; the port ends at the rows and switches
  the SPA reads and writes, plus the brief-nudge without its SSE publish.
  The prefs patch is the port's first zod record body — its full 400 table
  probed against the ui's own zod and pinned in Rust tests — and the route's
  ordering invariants held byte-for-byte against TS on the same DB: ids
  absent and ids empty both mean "mark all of mine", the member's delivery
  403 fires after the 400s and before any write (one PATCH never
  half-applies), and the admin flips write identical audit rows on both
  runtimes.

- **Task workflows speak Rust**: `/api/workflows` and `/api/workflows/{id}` —
  the list (any member; workflows ground what agents will be told), create,
  the field-at-a-time patch, and delete. The match/skills/toolkits payload
  rides as jsonb the DB itself orders, so both runtimes read the same bytes
  back; and the group's quiet corners held: there is no 404 (a missed id
  still answers ok), and an empty patch runs no SQL at all — which is why it
  answers ok even for a garbage `{id}` while a one-field patch takes the
  recorded platform-500 divergence. The zod array surface behind the body
  (element checks before array length, trim before length, unknown keys
  stripped at every level) is probed against the ui's own zod and pinned in
  the api's tests.
- **Teams speak Rust, and the identity-proxy model crosses with them**:
  `/api/teams`, `/api/teams/{id}`, and `/api/teams/{id}/members` — the list
  (with live member counts), create/rename/delete (owner-gated; boards
  survive team deletion as personal boards), and the member plane (add by
  email with re-role on conflict, remove that never removes an owner). The
  list is the port's first ACTING-user surface: a personal assistant calling
  with its `tak_` key acts as its owner, a general agent or a rejected
  credential resolves to nobody, and the owner's admin role gates the
  assistant's elevation. Byte-diffed against TS for anon, member, and admin
  callers plus both agent shapes — reads, write cycles, the 405s with their
  `allow` headers, and the full 400 table (the plain-Email check, the enum
  role with its `.default('member')`, and the uuid body member).
- **API keys speak Rust**: `/api/keys` and `/api/keys/{id}` — the list with
  its `canMint` gate, minting (the `tlk_` secret answered exactly once),
  revocation, and the self-imposed policy plane (#265: token/USD caps and a
  per-minute ceiling, where 0 means unlimited in the row but echoes back as
  the 0 its owner sent). The policy PUT is the port's first nullish
  `z.number()` body: the whole 400 table (safe-int guard bounds, a max
  breach that says "number" even on an int field, fractional-vs-int, type
  messages) was probed against zod 4.3.6 and pinned in Rust tests. Numbers
  now ride every Rust wire through one `js_num` helper (hoisted from the
  ledger) so an integral cap prints `1000`, never `1000.0`. Byte-diffed
  against TS for anon, member (no grant → 403), and admin callers, with the
  caps written by each runtime read back through both. One recorded
  divergence generalized into the migration doc: a non-uuid `{id}` (like
  TS's other uncaught route throws) answers the house 500 envelope, not the
  platform's plain-text sentence.
- **The admin console's first five groups speak Rust**:
  `/api/agent-role-templates` (built-in + own role templates, the shadowing
  rules), `/api/admin/password-accounts` (create/reset/remove — the password
  hashes itself never ride the audit log), `/api/admin/google-client` and its
  `/login` sibling (status, sealed client config, login toggle), and
  `/api/admin/instance` (the hosting domain and its self-fetch verification).
  `/api/admin/permissions` joins them, and its PUT body is the port's first
  zod union: probed against zod 4.3.6, only a string userId failing the uuid
  format check surfaces its own message (`Invalid UUID`) — every other
  both-branches-fail shape collapses to `Invalid input` — and that table is
  pinned in the Rust tests. All five byte-diffed against TS across anon,
  member, and admin callers, including the 405s with their `allow` headers,
  the full 400 table, and restore-safe write cycles on both runtimes over the
  same dev database. One recorded divergence (docs/RUST-MIGRATION.md):
  clearing the instance domain works from Rust and cannot from TS, whose
  driver writes JSON null as SQL NULL and answers the clear with a leaked
  Postgres constraint sentence.
- **The first product reads speak Rust**: `/api/agents` (the fleet list with
  tiers and per-agent access), `/api/apps` (the enabled-app manifest),
  `/api/activity` (the merged feed with the admins-only audit kind), and
  `/api/cost` (the full ledger overview — priced windows, per-model,
  per-agent, per-day) all serve from the Rust api, byte-diffed against TS on
  the same sessions for admin, member, and anonymous callers. The proxy gains
  an exact-match list alongside its prefixes: `/api/agents` and `/api/apps`
  are whole-path migrations because their sub-routes (`register`,
  `heartbeat`, the app-server gateway) stay TS until their batches.
- **A Rust-served budgeted call was flying blind**: the priced-view SQL that
  `spendSince` reads bound its cache multipliers as `$2`/`$3`, colliding with
  the caller's own second bind — the statement couldn't even prepare
  (`integer * text`), and the budget check's error-swallowing `.ok()?` turned
  that into "no spend data" on every Rust-served gateway call with a budget
  attached. The multipliers are values in the SQL text now, exactly as TS
  interpolates them; the statement prepares and returns real spend.
- **Research searches on proven capability, not on hope**: a model is only handed the
  search stage when something proves it can search — a probe or catalog that measured it
  browsing, or a checked search backend (SearXNG) the model can drive through a tool. An
  admin's assignment of a blind model used to be assumed native: the run asked plain
  weights for cited findings and got a fluent uncited brief (or a 502). Now that
  assignment is honored through the tool path, and a workspace with no proven search path
  refuses to start, naming what to connect (`NO_SEARCH_REASON`) instead of paying a model
  to answer from memory.
- The tool path is actually taken. `planSearch` always said which path a run would use,
  but only the model id reached the search stage — a `tool` plan was silently run as a
  native completion. The plan's supplier now rides the checkpoint and `searchStage` picks
  the transport to match.
- **A research run with nothing citable retries itself instead of parking on a person**:
  two more search rounds, then a failure with a sentence in `error`. It used to pause on
  a "search again?" decision that nothing in the UI could answer — an announcement landed
  as a notification while the run read "running" forever, which looked like a stall to
  everyone including the agent that filed the bug.
- **The token ledger counts what providers actually bill** (#243): cache-write, cache-read
  and reasoning tokens get their own `usage_events` columns, and `normalizeUsage` detects
  each provider's shape from the payload — Anthropic native reports cache tokens OUTSIDE
  `input_tokens` (the flat model understated), OpenAI-compatible folds cached input INTO
  `prompt_tokens` at full price (it overstated). Cache writes bill at 1.25× input, reads at
  0.1×; reasoning rides inside output, recorded for visibility. Volume views and the cost
  cards total every kind now, with a standing "rate-card estimates, not invoices" footnote.
- Gateway metering no longer escapes on abort: the streaming ledger write settles exactly
  once from flush, client-cancel, or the request's abort signal — a client that hangs up
  mid-stream is still billed by the provider, so it is still recorded. Non-streaming metering
  books usage from failed responses too (a rejection without usage books nothing — a
  rejection can't invent spend).

### Added

- **Google sign-in speaks Rust too — and with it the session batch is whole**:
  `GET /api/auth/google` and its `/callback` are serving from the Rust api, the
  OAuth dance hand-rolled over reqwest (no SDK — the identity comes from
  Google's userinfo endpoint, never a locally-verified JWT). The consent URL
  byte-matches TS's own `URLSearchParams` serialization (pinned by test, then
  diffed live against the TS serializer with the same client record and state
  token), the state cookie is the same double-submit CSRF token with its
  10-minute TTL and constant-time compare, and every failure door bounces to
  `/login` with the machine-readable reason the SPA already renders. Verified
  against the live dev stack: the disabled gates byte-diffed equal on both
  routes; the client-precedence rule (a complete Admin record beats env, and
  the env pin is what enables login — the record alone never does) exercised
  for real; bad-state and cookie-mismatch bounces equal; and a live token
  exchange against Google with a garbage code returns the fixed
  `exchange_failed` sentence with the provider's prose dead in the log — the
  boundary rule, held. The happy path past Google's consent screen (claim,
  org-domain gate, invites) needs a human signing in, so those doors are
  verified to their last reachable hop, not past it. The coexistence proxy now
  forwards the caller's origin (`x-forwarded-proto`/`host`) so the two runtimes
  derive the same `redirect_uri` through it.
- **Password login, the login screen's door list, and the first-run claim speak Rust too**:
  the session batch's second slice — `POST /api/auth/password`, `GET /api/auth/providers`,
  `POST /api/auth/claim` — is serving from the Rust api. The credential check is the same
  scrypt contract both runtimes share (`scrypt$N$r$p$salt$hash` entries, parameters read
  from the entry, the dummy-hash burn that keeps an unknown email exactly as slow as a
  wrong password), run on the blocking pool where ~100ms of KDF never parks an async
  worker. Zod's exact 400 messages ride through verbatim (probed against the ui's own zod
  4.3.6 — the email pattern hand-rolled, and its truth table pinned), the login brakes
  share one Redis counter space with TS (a Rust attempt and a TS attempt spend the same
  budget; a success resets it), and the claim's advisory lock still makes a lost race a
  409. Verified against the live dev stack: providers byte-identical between runtimes; a
  Rust login verifying a node-hashed entry and returning the login body byte-identical to
  TS's; either runtime's logout destroying the other's session; the claim limiter's 429
  (message and Retry-After) agreeing across the shared counter; every validation error
  path diffed equal.
- **The session plane now speaks both runtimes**: the second port batch's first slice is
  serving — Redis `sess:<sid>` sessions (the shared store both runtimes read while the
  port runs), `GET /api/auth/session` (user + denied views + effective permissions, read
  from the DB so an admin's change lands without re-login), `POST /api/auth/logout`, and
  `GET /api/users` — the fleet-wide auth oracle every `mcp/` agent authenticates through,
  ported with its agent-caller resolution: per-agent `tak_` keys where the key proves
  identity and `x-agent-name` can only narrow it, and the org-wide legacy key resolving
  identified-but-untrusted while refusing outright a name that carries human privilege.
  The proxy now forwards `Set-Cookie` plural-safe (a comma-joined multi-cookie header is
  a corrupt one, and an auth plane that cannot clear its cookie through the boundary is
  not ported). Verified: `/api/auth/session` and `/api/users` responses byte-identical
  across the TS and Rust servers against the same Redis sessions (admin, member,
  anonymous); a Rust-served logout observed immediately by TS; the oracle's accept /
  wrong-name-refusal / 401 paths against a real `tak_` key, `last_used_at` landing and
  the row restored after.
- **The API is moving to Rust, and the first slice is already serving**: a new `api/`
  crate (axum + sqlx) ports the backend batch by batch while the app keeps working — a
  loopback proxy in the TS server forwards migrated prefixes to it
  (`TALARIA_RUST_API_URL`; unset is the byte-identical TS-serves-everything default, and
  there is deliberately no fallback when it is set). Batch 1, the LLM gateway
  (`/api/llm/v1/*`), is done: models and chat/completions — streaming included, with the
  relay metering tokens and booking the ledger whichever end the stream comes to, so a
  client that hangs up mid-stream is still recorded because the provider still bills —
  per-key caps and throttling, rate limits, secretbox unseal pinned by two-way
  cross-language vectors, and the confab guard riding the relay in all four postures
  (observe records, annotate warns in-stream, strict scrubs). `talaria dev` gains
  `TALARIA_API=on` to raise the api as a sidecar (adopting one already on the port,
  degrading to a warn on a box without cargo), the devbox image carries the pinned
  toolchain, and CI gates the crate (fmt + clippy + the pure unit suite —
  service-dependent tests stay local, by the house rule). Roadmap and coexistence rules:
  `docs/RUST-MIGRATION.md`. Verified: models byte-diffed against TS on the same dev DB;
  live streaming and non-streaming completions through a real provider with
  `usage_events` and `guard_findings` rows checked after each guard posture, including a
  strict-mode redaction of a model-invented credential; the sidecar's spawn, adopt and
  no-cargo paths; the devbox image building the whole crate, C toolchain and all.
- **Creating an agent is a hire that runs in the background, not a request the modal has
  to babysit**: "Create agent" now enqueues a durable `agent-hire` run (create the def,
  write starter skills, render the fleet, boot the container, wait out the healthcheck)
  and closes immediately. The roster shows the hire working through its phases — the
  run's own sentences (`rendering the fleet config`, `starting the container`), no fake
  percentage — and the finished agent's tile materializes over the strip without a
  refresh; a failure shows its sentence for ten minutes. A boot that runs to minutes on
  a cold pull used to live inside one POST: a modal that couldn't close, proxies timing
  out, and agents visible only after a reload nobody knew they needed. The one error the
  open modal still owns is a taken handle (409, fixable in place). Role templates moved
  onto the first step of the flow — "or start from a role" fills the review form and
  jumps to it, so a fresh install's entry path no longer hides behind a form that only
  exists after you describe an agent first.
- **A personal assistant can work the boards it is told it owns**: a board is born
  carrying its creator's personal assistant on the agent allowlist, and a new assistant is
  seeded onto the boards its owner already has — in both cases inside the same breath as
  the creation, so there is no window where `list_boards` shows the board under the
  owner's role while every board-scoped route 403s it. Boards the owner does not own stay
  the board owner's call, and a removal via `set_board_agents` is never re-added: the
  propagation lives in the seeding, not in the gate.
- **A run that needs a person asks on its own page**: a parked run's question renders in
  place on the research surface — with the options the step itself offered — and answering
  there resumes the run (`POST /api/research/:id/decide`, authority-checked by the run's
  declared audience; a member reads the question, the owner answers it). Status stays
  four-value on the wire; the question rides beside it as `awaiting`, null the moment it
  is answered.
- **LLM spend ceilings** (#243): rolling-window budgets in Admin → Settings — org-wide and
  per-caller, in tokens and/or priced dollars — checked before every gateway call (the HTTP
  route answers 429 `budget_exceeded` with `retry-after`; internal callers like the QA judge
  are held to the same ceiling). Off by default; a $ ceiling is never tripped by tokens with
  no price configured, and spend reads are cached briefly except at the edge (>80% of a cap
  goes exact, bounding what a burst can slip past). Cron schedules get a frequency floor
  (default: nothing faster than every 5 minutes) — a cron is an agent turn is spend.
- **Per-key caps and throttling** (#265): each LLM-gateway key carries its own ceilings in
  **Settings → API keys** — a spend cap (tokens/$ over the org budget window) and a
  requests-per-minute limit. Both self-imposed and unlimited by default; the cap can only
  tighten an admin's ceiling, never raise past it, and the refusal (`429 budget_exceeded` /
  `rate_limit_exceeded`, with `retry-after`) names the surface that holds the number.

### Security

- Upstream error text is sanitized at the trust boundary (#268): the two
  proxy wires — `/api/llm/v1/chat/completions` (external key holders) and the
  MCP gateway (agent containers) — no longer forward an upstream's error body
  verbatim. Failed hops answer with the status (ours to share) and a fixed
  sentence; the only upstream-written text that survives is the structured
  `error.type`/`error.code` tokens OpenAI-style clients switch on for retries,
  length-capped. The verbatim body goes to the server log, inside the
  boundary. JSON-RPC errors on the MCP wire ride 200s and pass untouched —
  tool results, including tool failures, are the protocol the agent speaks.
- Oversized uploads are refused before they are buffered (#266): the upload
  route reads its multipart body through a capped stream — a declared
  content-length over the cap is answered 413 from the header alone, and a
  chunked body (what every browser FormData POST is) is aborted mid-read at the
  cap. Previously the whole body was buffered and only saveUpload's byte count
  refused — after the memory was already spent.
- Compose generates its first-boot secrets (#267): `talaria deploy up` writes
  `POSTGRES_PASSWORD` and the minio root pair into `docker/.env` (once, 0600,
  git-ignored) before invoking compose — the two places no container entrypoint
  can reach, because interpolation happens at container-create time. An existing
  postgres volume keeps the password it was initialized with (a fresh random
  would lock the app out); the file says how to rotate. Without this, an
  unconfigured compose instance ran on passwords published in the repo.
- Password credentials live in Postgres as scrypt hashes (#244): `user_password_credentials`
  stores `scrypt$N$r$p$salt$hash` (node:crypto, params in-band) — never plaintext, never env.
  A login miss on the email burns a dummy verify, so response timing can't reveal which
  addresses have accounts.
- Google sign-in refuses unverified email addresses (#269): an identity whose email claim
  Google has not verified — including an absent claim — is rejected at code exchange, so an
  unverified address can no longer mint an account.
- **Compose's env channels are git-ignored**: `compose.env`, `compose.override.yml` (now the
  devbox carrier for provider tokens and `--env` secrets) and `docker-compose.override.yml`, as
  bare patterns so a devbox tree relocated into a checkout (`TALARIA_DEVBOX_HOME`) is covered
  too. A regression test pins the canonical secret-carrying paths to `git check-ignore`.
- **2026-08-26 audit remediation** (full work order: `docs/history/AUDIT-2026-08-26.md`, PR series
  #257–#265). The one real vulnerability: both bytes routes served uploader-declared MIME inline —
  `text/html`/`image/svg+xml` executed same-origin with the viewer's session. `serveUpload()` is
  now the single disposition decision (raster + PDF inline; everything else attachment + nosniff +
  sandbox CSP). Also: credential sealing now walks array-content turns (image turns traveled whole
  — the exact case the adversarial tier measures); the one raw fetch on a model-supplied URL goes
  through safe-fetch; federation imports validate slug/department alphabets (newline → `.env`
  injection, `../` → traversal); invite email HTML escaped; timing-safe OAuth state compare; MCP
  gateway validates non-builtin upstream URLs; `/api/join` rate-limited. Deferred hardening filed
  as issues: llm.v1 per-key spend caps, stream-based upload size rejection, compose first-boot
  secret generation, upstream-error-text sanitizer pass, Google `email_verified` enforcement.

### Breaking

- **Self-hosters — env auth is removed entirely.** `AUTH_USERS`, `AUTH_PASSWORD_ENABLED` and
  `AUTH_ADMIN_EMAILS` are ignored after this upgrade (no import path). A fresh instance has
  zero users: the first visit offers `/claim`, and the account created there (email + password,
  or the first Google sign-in when Google login is enabled) becomes the admin — first claim
  wins, so reach the claim screen before exposing a fresh instance publicly. Existing users
  keep their stored roles; re-create password sign-in via Admin → People (or Google).
  `AUTH_ALLOWED_*` gates Google sign-ins only now — password accounts are admitted by the
  admin who creates them.
- External SDK consumers only: API renames — `/api/plan/:id/doc|draft` → `/api/plans/…`; singular
  `/api/inbox/focus/conversation` folded into `/api/inbox/focus/conversations/$id` (with `current`
  sentinel); `/api/profile` → `/api/me`. Validation-failure bodies are now zod-issue 400s across
  ~90 routes (was a mix of 400/422/custom); 401/403 split fixed on four task endpoints; 500s no
  longer echo `e.message`.

### Added

- **First-run claim + admin-managed auth.** A fresh instance offers `/claim` (email + password,
  plus "Claim with Google" when Google login is on); the first identity through becomes the
  admin, advisory-lock serialized so a race can't mint two. From then on auth is managed in the
  app: Admin → People gains **password accounts** (create, reset, remove — including your own),
  roles live only in the database (a sign-in never changes one; the last admin can't be demoted),
  and once a Google client is configured, Admin → Google client links the admin's own Google
  account.
- Google login can be enabled from the Admin UI (Admin → Google client): the toggle writes the
  `google_login_enabled` setting (`PUT /api/admin/google-client/login`), so login no longer needs
  `AUTH_GOOGLE_ENABLED=1` in the env. The env var still pins login ON (undeactivatable from the
  UI); the toggle stays inert until a client is actually configured.
- `talaria service` — `install` starts the production stack (the `deploy up` build, in your
  terminal) and installs a systemd unit that starts it at boot, health-gates that start on the
  compose healthchecks (`up -d --wait`, dropped automatically on compose builds that hang on it),
  and stops it cleanly (`compose down`) before docker.service; `uninstall` removes both, keeping
  volumes and state; `status` shows the unit state and the compose view. `install` also pins
  `DOCKER_GID` into `docker/.env` — the boot unit has no shell to resolve it from
  (docs/CONTAINER.md → "Keep it running across reboots").

### Changed

- **Devboxes spawn configured for the creating shell**: `box new` inherits
  `ANTHROPIC_BASE_URL`/`ANTHROPIC_AUTH_TOKEN` (and `_MODEL` when set) into the box's
  `compose.override.yml` — a shell whose own Claude runs on GLM gets boxes that do too, no
  per-box login. `--env KEY=VALUE` (repeatable) carries any other container env (explicit keys
  beat the inherited trio), and `--setup <cmd>` (repeatable) runs arbitrary provisioning inside
  the fresh box. An explicit `--claude-token` disables the inheritance; the two auth vars never
  ride together.
- **A harness of choice installs once, not per box**: every box mounts the shared tools layer
  (`../devboxes/shared/tools` → `/work/tools`, its `bin` first on PATH). `talaria box install
  <name> '<cmd>'` installs into it from any box — flock-held, `NPM_CONFIG_PREFIX` pointed at the
  layer (the image's own global prefix is root-owned; plain `npm i -g` in a box was always an
  EACCES) — and the result is usable from every box, survives `box rm`, and rides into boxes
  created later. `--setup` hooks get the same tools env, so recreate-with-same-`--setup` is a
  no-op, not a re-download.
- **One owner per duplicated helper** (the audit's recurring failure mode): Google OAuth built once
  (`server/google/oauth.ts`), JSON-RPC envelope once (`mcp-jsonrpc.ts`), `errText`/`errLine`,
  `tz.ts`, `docker-exec.ts`, `asIso`, shared zod schemas (`lib/api-schema.ts`); `localMoment`,
  board-visibility SQL, MCP protocol pin consolidated. Client mutations all through
  `fetch-json.ts` (`postJson`/`putJson`/`patchJson`/`delJson`) — and the `credentials:
  'same-origin'` stanza is now census-enforced to live only there.
- **UI primitives close their gaps**: new `Popover` shell (nine hand-rolled engines deleted onto
  it), `IconButton size="tile"`, `Checkbox bare`, `StatCard href`, `StatusDot color`, Board's view
  toggle on `Segmented`; one emoji picker (chat's hand-rolled grid deleted); ~80 icon-button,
  checkbox, dot and input sites adopted. `SaveButton` deleted (0 uses; `useSavedFlash` stays — it
  has 8 importers).
- **Teams**: `renameTeam`/`deleteTeam` landed — owner-gated route + minimal UI + the functions'
  first tests.
- **Dead code out**: `adapter/`, `stack/`, the unreachable focus-inbox cluster (11 files),
  `MentionMenu`, `EmojiShortcodeMenu`, `NotificationsPanel` (live toasts are `NotificationToasts`),
  `native-tools.ts` — every deletion grep-verified. Docs now say what's real: the plugin is
  dormant/manual, backup is `bun talaria backup`, the work order lives in `docs/history/AUDIT-2026-08-26.md`.
- **Storage**: production refuses the published dev S3 password at the internal-mode use-time
  doors (env.ts promised this guard; it didn't exist until now).
- check-invariants grew three tripwires: inline-serving only via `serveUpload`, same-origin-fetch
  census at zero, popover census.

### Fixed

- **A Google login for an email that already has an account lands on that account —
  no more forked second user.** Sign-in resolved people by sub alone, and subs are
  per-door (`google:<subject>` vs `password:<email>`), so a Google login for a
  claimed email silently inserted a second `users` row with role `member`: the
  person's admin powers stayed on a row Google could never reach, and every Google
  session ran as the fork — `users.email` has no unique constraint, so the insert
  never failed. Sign-in now links by verified email first (Google refuses
  unverified addresses, and the password path already links by email — the same
  trust, the missing half), adopting the Google sub onto the existing row without
  touching its role; a fork the repair hasn't merged yet can't have its sub stolen,
  and the first-claim path promotes a same-email member to admin instead of
  forking. Existing deployments heal at boot: a one-shot migration merges every
  forked pair — admin row survives, everything pointing at the fork is re-pointed
  or dies in its cascade — before the api starts. Merged-away rows' sessions age
  out with the 7-day Redis TTL; those people sign in once more and land on the
  survivor. Deployments need a rebuilt image; the repair runs at boot, no operator
  action.
- **Home's "Set up your assistant" opens Settings → Assistant** (it went to
  `/agents`, a Manage view members are denied — the route gate bounced them
  straight back, so the button flashed and did nothing).
- **Google login no longer dumps back to /login — every `Set-Cookie` survives the
  HTTP boundary.** The Response → `res.writeHead()` conversion in both wrappers
  (prod `server-entry.js`, the vite dev middleware) read headers with
  `Object.fromEntries(response.headers.entries())`, which collapses duplicate
  keys — and `Set-Cookie` is the one header that legitimately repeats. The
  OAuth login callback answers with two cookies (session on, one-shot state
  off), so the object kept only the state clear and the session cookie died at
  that hop on every deployment since the prod server landed: Google authorized,
  the SPA booted at `/`, `/api/auth/session` said `{user: null}`, and the
  cockpit bounced to /login — while password login (one cookie) worked, and
  dev never caught it because the middleware had the same one-liner. The
  conversion now lives once as `writeHeadHeaders` in `ui/src/server/http.ts`
  (re-exported through the server bundle beside `migrate`), both wrappers
  import it, and a test pins that a two-cookie response stays two cookies.
  Deployments need a rebuilt image to pick this up — no env or Google-console
  change is involved.
- **The auth gate no longer flashes the dashboard before login.** The app shell
  painted its skeleton (rail, strip, content cards) while the session read was
  in flight and in the beat before the /login navigation landed — fine for a
  signed-in reload, the dashboard flashing at every signed-out visitor. The
  gate now holds on the Mercury ground with the brand mark centered until the
  session resolves; a failed read keeps the real chrome with its retry, as
  before.

- **`box start` now converges with `up -d`** (was `compose start`, which merely re-launches the
  existing containers): the documented stop → edit `compose.override.yml` → start flow for
  auth/env changes silently never applied. Regression-tested against the verb.

### Documentation

- **Docs overhaul, part 1 of 5 — restructure + truth** (first PR of the docs pass: dev docs and
  end-user docs, succinct and table-first). Ten dated/superseded docs moved to
  `docs/history/` (PLAN, PRICING, SECRETS-PLAN, PRODUCT-GAPS/PRODUCT-PLAN, three audits,
  m0-contract, TODO) with a README stating the rule: nothing there is current, the live doc wins —
  every inbound link repointed. New `scripts/check-docs.mjs` tripwire: every markdown link in the
  tree must resolve to a real file (would have caught the `m0-contract → adapter/` link that
  pointed at a deleted directory for months); wired into `bun run check`. `apps/README.md`
  rewritten to the real Svelte 5 SDK anatomy (was React-era: tsx/useQuery/react imports);
  `ui/README.md` setup section now shows the one true path (`bun talaria setup` from the repo
  root), drops its roadmap-duplicate sections, and documents per-agent `tak_` credentials instead
  of the retired org-wide `TALARIA_AGENT_KEY`.

- **Docs overhaul, part 2 of 5 — the generated references** (`bun run docs:api`, wired into
  `bun run check` as `--check`). New `scripts/gen-docs.mjs` extracts the HTTP API reference from
  the route sources — 214 routes, 343 (path, method) rows → `docs/api/` (23 group files + index
  with the auth legend): path, method, guard class (session/admin/perm/view/agent/dual/fleet/
  bearer-key/public), parseBody field tables (inline and named zod schemas, z.union variants),
  literal statuses, SSE/audit markers, and a heuristic Returns column that prints `…` rather than
  guess. The CLI reference (`docs/CLI-REFERENCE.md`) imports the real command tree — the same
  declarations `--help` renders — plus a hand-written guide (`docs/CLI.md`). Prose notes live in
  the source as `// doc:` runs (21 routes seeded); otherwise a route's own leading comment is the
  note. Guard vocabulary is closed: an unrecognized `await X(request)` renders `unknown(X)`, never
  a false `public` (this caught `actingUser`/`taskActor`/`commentReader` during the audit). CI now
  runs `bun run check` instead of the bare invariants script, so every gate added to the chain
  actually reaches CI.

- **Docs overhaul, part 3 of 5 — the full SDK docset.** `docs/sdk/` replaces the single-file
  `docs/SDK.md` (now a redirect stub, so existing links survive): getting-started, ui-kit,
  client, server, mcp, harnesses (including the previously undocumented bridge pattern:
  `runHarness`/`resolveHarnessModel` from app code), workbench-harnesses (the two-contracts
  table), and `reference.md` — every export of both entry points, one row each. New tripwire in
  `check-docs.mjs`: SDK export coverage diffs both directions against `reference.md`, so a new
  export without a row fails `bun run check`, and so does a documented symbol that doesn't
  exist (this caught `KeyHint`, which the old doc listed for months). Surface repairs the
  coverage pass surfaced: `Popover` now actually exported from `@talaria/sdk`;
  `CheckResult`, `ToolPolicy`, `ModelChainStep`, `ResolvedHarnessModel`, `RunLedger` exported
  from `@talaria/sdk/server` (all referenced by already-exported types). The harness example
  and the bridge example in `docs/sdk/harnesses.md` are transcribed verbatim into
  `ui/src/sdk/server.test.ts` and run through the real runner — the documentation is a build
  target. `docs/APPS.md` rewritten to the Svelte 5 anatomy, linking the docset.

- **Generated API reference: 22 hidden method rows recovered.** The generator's method-span
  scanner ran over raw source, so a comment inside a handler whose text contained backticks
  and an apostrophe (`` `channels.ts`'s ``) read as a phantom string to the scanner and
  de-synced its brace depth — `POST /api/channels/{id}/messages`, `PUT/DELETE
  /api/tasks/{id}`, and 19 more rows never rendered (their body schemas attached to the
  wrong row). Comments are now stripped length-preservingly before scanning, so offsets and
  `// doc:` note positions survive; 343 → 365 method rows across 18 group files.

- **Docs overhaul, part 4 of 5 — the member guides** (`docs/user/`). Eight chapters plus an
  index, one per work surface, written for people who use Talaria rather than run it:
  getting-started (sign-in, the assistant wizard, finding your way), your-day (the daily
  brief, the assistant drawer, notification classes), comms (channels, relays, DMs, agent
  chats), boards (tickets, workflow columns, saved views, review and sign-off), plan (the
  multiplayer planning conversation and its living document), research (three depths of
  cited runs), knowledge (spaces, official content, OKF), files (places, sharing, Secrets).
  Every recipe uses the UI's own words — the labels, buttons, and empty-state sentences the
  components render — traced to source while writing; the agent chapters and admin guide
  follow in part 5.

- **Docs overhaul, part 5 of 5 — agents, admins, and the glossary** (`docs/user/`). Nine more
  chapters complete the member docset: working-with-agents (meeting agents everywhere —
  mentions, chats, board tickets, the review walk, the rails), personal-assistant (the
  wizard, tuning in Settings, the brief and drafted replies, Google connection and
  delegation), templates (the ticket/plan skeletons and the resolution chain), apps
  (lifecycle, grants, the shipped reference apps), four admin chapters (people with the full
  permission catalog; the agent roster; models; MCP + observability + apps), and a glossary
  covering every term the guides use. Cross-links landed at the seams: comms and boards now
  point into working-with-agents, and getting-started's read-next covers the whole set. The
  permission table is verified id-for-id against `server/permissions.ts` (13 permissions,
  same order, same member defaults).

- **Docs overhaul, closeout — the developer hub.** New `docs/ARCHITECTURE.md`: the two planes
  (one app process + the stateless MCP proxy) with the port table, the request lifecycle
  (including the honest note that CSRF protection is SameSite=Lax + OAuth state, not
  middleware), the auth stack (sessions, `tak_` agent keys, the legacy-key refusal, `tlk_`
  bearer keys), the data layer (append-only checksummed migrations, envelope-encrypted
  secrets, numerics-as-strings), the realtime bus and its "says what changed, never what it
  says" rule, rendered fleet + blue/green rolls, the 51-tool MCP plane and its server-side
  guardrails, the gateway as the single enforcement point, compile-in apps, and the
  prod-only scheduler. `DEVELOPERS.md` rewritten as the complete hub — every doc in the repo
  listed and grouped, nothing orphaned; `CONTRIBUTING.md` slimmed to the rules that aren't
  style; the old `HANDOFF.md` moved to `docs/history/` behind a stub, and the completed
  2026-08-26 audit with it. Truth fixes the pass surfaced on the way out: routing is the
  `defineApi('…')` literal, not the filename (API-CONVENTIONS said the dot rule was
  load-bearing; it's convention); dev infra is six services, not two (Postgres, Redis,
  Qdrant, TEI, MinIO, SearXNG); `mcp/`'s guardrail described as what it actually is — no
  assignee writes, no terminal status moves.

- **Docs refinement pass — the member guides written for readers, not gates.** The one
  systematic artifact of the docset's authoring formula is gone: every per-chapter "Words"
  dictionary table deleted (11 chapters) — the glossary defines each term exactly once,
  and where a deleted table held an insight the chapter needed (distill's summary landing
  in Files, the fitness sense of "tier" vs an agent's model alias, the brief/digest pair,
  the role-templates disambiguation) it became a sentence where it's used. Chapters now
  close on their own strongest note instead of the same dictionary; the docset index says
  what a reader gets rather than describing the house style; one-sentence mega-constructs
  (the assistant's Schedules/Skills/Memory parenthetical, the permission-chip
  instructions, the ticket-detail run-on) are broken into readable shape. Same facts,
  same traced claims, same coverage — the link check and full verify stay green.

### Added

- **Release channels: nightly, RC, and stable images on GHCR.** A `rc`
  branch stages release candidates, a `testing` branch feeds a nightly
  (03:17 UTC), and one release workflow publishes both plus the stable
  path: a `vX.Y.Z-rc.N` tag on `rc` builds image `X.Y.Z-rc.N` + moving
  `rc` and opens a GitHub prerelease; a `vX.Y.Z` tag builds the version +
  moving `latest`; the nightly publishes `nightly` + `nightly-YYYYMMDD`
  with no Release (365 prereleases a year is tag noise with no reader).
  The channel resolution lives in one `resolve` job that fails loud on
  malformed tags — nothing silently falls through to `latest`. Publishes
  are gated by calling ci.yml as a reusable workflow, so "what gates a
  release" and "what gates a PR" are the same list and cannot drift. Git
  tags are the version authority (the `package.json` versions stay
  decorative and unread); the workflow never commits a version bump. The
  image carries its identity as OCI labels + `TALARIA_VERSION`
  (`unknown` on local builds, which is simply true of a local build).
  Operators consume a channel with a committed override —
  `docker compose -f docker/compose.yml -f docker/compose.registry.yml
  up -d --no-build` with `TALARIA_CHANNEL` pinning the tag — and the
  default checkout-build path is byte-identical to before (the base
  compose file changed comments only, verified by diffing
  `docker compose config`). The model, the tag grammar, and the
  one-time GHCR visibility flip: `RELEASING.md`.

- **`talaria` — one TypeScript CLI for everything the bash scripts did.**
  ~1,600 lines of shell across eleven scripts are now a zero-dependency
  `cli/` package run directly under bun (`bun talaria …`), with a flat
  tree: `setup`, `dev`, `worktree`, `reset`, `box`, `deploy`,
  `backup`/`restore`. Every ported script is deleted in the same commit
  that replaces it and every doc reference rewritten — no shims, no dual
  sources of truth. The CLI is dependency-injected end to end
  (`run(ctx, args)` with exec/pipe/readLine/env/isTTY/now behind one Ctx
  interface), so all 126 of its tests run against planted process
  behavior with no mocks. Safety rails survive structurally rather than
  by careful quoting: reset's SQL runs as `psql -c` argv with
  `ON_ERROR_STOP=1` (the bash's stdin-swallowing `read` trap is
  impossible now), restore streams `gunzip | psql` (dumps never sit in a
  buffer), and secrets land only in 0600 files. Two latent bash bugs the
  port's live round-trips surfaced are fixed: worktree creation died on
  container-name conflicts with the main stack whenever the main stack
  was up (its compose up is now scoped to the two services a worktree
  actually owns), and `dev` inside a worktree silently adopted the MAIN
  stack's containers under the default compose project (it now aims at
  `talaria-wt-<name>` with ports lifted from the worktree's own ui/.env).
  `deploy` wraps exactly the commands CONTAINER.md documents and prints
  each one before running it — a test reads the doc at runtime and
  asserts the argv is character-for-character identical, so the doc and
  the CLI cannot drift; it also resolves `DOCKER_GID` from the socket and
  warns when a shell export is missing from `docker/.env` (the drift trap
  the doc warns about, detected before it bites). `setup` now also installs
  a bare `talaria` command on your PATH — a two-line sh shim in bun's bin
  dir that runs the invoking checkout's CLI from anywhere (re-running
  setup elsewhere repoints it; `TALARIA_BIN_DIR` overrides the location),
  with a warning when the target dir isn't on PATH. Legacy checkouts:
  re-run `bun talaria setup` to repoint the `git wt` alias at the CLI.
  The runtime-coupled scripts (`update-restart.mjs`, `chassis.template.yml`,
  `skills/`, the smoke/invariant checks) stay where they are — CI and the
  prod image call them by path.
- **Devboxes: a containerized dev environment per task.**
  `./scripts/devbox new <name>` builds a disposable full stack per agent
  session or experiment: a local clone of the repo on its own branch
  (`agent/<name>`), private stateful sidecars (Postgres/Redis/Qdrant/MinIO)
  seeded from the primary dev environment, a private fleet project, and the
  agent CLIs — Claude Code *and* opencode — inside the container
  (`./scripts/devbox enter <name> claude|opencode`). The toolchain image
  (`docker/devbox.Dockerfile`) is glibc on purpose (bun + the npm-packed CLIs
  are glibc-first), fixes uid 1000 to match the host user so the bind-mounted
  clone needs no chown dance, and pins the CLIs with their autoupdaters off
  (version bumps are rebuilds). Networking is the production shape applied to
  dev — three networks per box: its own (sidecars by service DNS, nothing
  published — several are unauthenticated), the primary stack's network
  (shared stateless TEI/SearXNG, reached by container name), and a per-box
  `devbox-<name>-fleet` the spawned agents attach to, dialing the app
  container-to-container (`TALARIA_AGENT_DIAL=container`). State binds at the
  same absolute path on host and container, the same rule as the production
  deploy, because the fleet renderer bakes host paths into agent binds; all
  paths are canonicalized so symlinked checkouts can't leak a spelling the
  box can't resolve. Seeding is a snapshot, not a link: Postgres dump-restore
  and an `mc mirror` of the bucket always (Qdrant is opt-in `--qdrant`, being
  derived data; Redis never, being transient), with the primary's
  `TALARIA_SECRET_KEY` carried verbatim so sealed secrets decrypt. Agent-CLI
  auth is per-box in a named home volume — interactive login, a headless
  `--claude-token`, or GLM/provider env via a 0600 `compose.override.yml`
  (`ANTHROPIC_BASE_URL`/`AUTH_TOKEN`/`MODEL`; never together with
  `CLAUDE_CODE_OAUTH_TOKEN`) — a host `~/.claude` is never shared, because
  concurrent CLIs corrupt `.claude.json`. `rm` refuses unpushed work;
  teardown removes projects, volumes, the fleet network and the directory
  with no residue. Verified end to end: two boxes plus the primary ran three
  fleets on one host, both apps answered on their 53xx ports, agents reached
  their app by service name with zero published ports, and home-volume
  logins survived stop/start. Runbook: docs/DEVBOX.md.
- **A single-container deploy: production image + instance compose.** The
  whole app now builds into one image (root `Dockerfile`, multi-stage:
  bun/node build → pruned prod deps → an alpine runtime with just bun, the
  docker CLI + compose plugin, and git) and one `docker/compose.yml` stands
  up an instance — app plus postgres, redis, qdrant, TEI embeddings, minio
  and searxng, nothing published except the app itself. Configuration is
  env-only by design: the image ships no `ui/.env`, real environment always
  wins, and the entrypoint (`docker/entrypoint.sh`) generates what's missing
  — secrets into `/var/lib/talaria/env/generated.env`, first-boot admin
  credentials into the logs — so `docker compose up -d --build` with zero
  env lands on a working, signable instance, while an orchestrator that
  supplies everything overrides all of it. State is a host bind mounted at
  the same path on both sides, because the fleet renderer bakes absolute
  host paths into agent bind mounts that the *host* daemon resolves — a
  named volume would be invisible to every agent. The fleet goes
  container→container: the app joins the shared `talaria` network (a new
  `TALARIA_AGENT_DIAL=container` makes the manifest dial agents by compose
  service name instead of host-loopback ports; `TALARIA_MCP_GW_URL` and
  `TALARIA_GATEWAY_SELF_URL` point agents at the app's service name), which
  implements the "right shape" AGENT-NETWORKING.md describes — no host
  firewall rule, and agents can reach the app but never postgres. The
  fleet preflight derives its probe target from the renderer's config
  instead of a hardcoded `host.docker.internal`, so containerized instances
  stop reporting false negatives. SearXNG's settings (secret included — it
  ignores env) are rendered by a one-shot init service running the same
  image, and the updater stands down (`TALARIA_UPDATER=off` baked; deploys
  are rebuilds, Dokploy/Portainer/plain-compose all work — build-at-deploy
  time from a checkout, registry publishing left as a commented `image:`
  line). Runbook and env contract: docs/CONTAINER.md.
- **A golden-image deploy for Proxmox.** `scripts/image/` builds an openSUSE
  MicroOS template with the system half of an install baked in — Docker +
  compose v2 (podman stays unused), Tailscale, firewalld rules for the
  agent→app path from AGENT-NETWORKING.md, Bun, the guest agent — and no
  Talaria in it at all: every instance cloned from the template installs
  *current* Talaria on its own first boot (clone → setup.sh → infra → build
  → `talaria.service`), so the image can't ship a stale app. Per-instance
  configuration rides cloud-init snippets (`qm set --cicustom` →
  `/etc/talaria.env`): a handful of vars steer the bootstrap (tailnet key,
  instance hostname, repo, ref) and everything else reaches the app process
  verbatim, winning over `ui/.env` the way the environment always has.
  `TALARIA_HOSTNAME` names the whole instance — system hostname via
  hostnamectl plus a hosts(5) entry, not just the Tailscale node name —
  because the cicustom snippet replaces the user-data that would otherwise
  carry one, and every clone would answer to the template's neutral name.
  The install is
  re-entrant — first boot retries converge instead of wedging on a
  half-installed `node_modules` (setup.sh's skip-if-exists can't heal that
  on its own), the app unit gates on Postgres readiness rather than losing
  the boot race to a cached failed migration, and a re-run never deletes the
  checkout, where uploads and fleet state live outside git. systemd owns
  restarts, so the in-app updater stands down (`TALARIA_UPDATER=off`) and
  updates are a one-liner. The SearXNG settings render moved from dev.sh
  into a shared `scripts/render-searxng.sh` so the bootstrap and the dev
  loop mount the same file. `build.sh` assumes nothing about the host it
  runs on: it prompts for the VM-disk storage (from live `pvesm` output)
  and the snippet storage (offering to enable snippets on `local` by
  *merging* content types — `pvesm set --content` replaces the list), and
  when the host has no SSH public keys it generates and names a keypair to
  inject; the same choices exist as flags for non-interactive runs, and
  `--dry-run` prints the resolved plan. Full runbook:
  docs/SELF-HOSTING.md. Verified repo-side: `bash -n` on every new script,
  the generated cloud-init snippet round-trips its four embedded files
  byte-for-byte under YAML literal-block indentation, and the build.sh
  resolution/prompt/keygen flows pass a stubbed-host test matrix
  (`pvesm`/`qm` stubs, pty-driven prompts, dry-run assertions); the image
  build itself runs on the Proxmox host per SELF-HOSTING.md (not
  exercisable from this tree).
- **Apps get their own place in the sidebar.** The rail now separates enabled
  apps from Work: Work stays Talaria's own surfaces (Inbox, Comms, Boards, and
  the rest), and each app's work surface sits under a new Apps heading of its
  own between Work and Manage — you can tell platform from app at a glance.
  An app's manage surface still slots under Manage, because Manage is the
  control plane no matter who published the view. The Apps heading only
  appears when an app is enabled; with none installed the rail reads exactly
  as it did. Verified live both ways: no apps enabled (no phantom heading)
  and the contacts reference app on (Apps holds it, its manage row lands
  under Manage, and `/x/contacts` still routes).

- **Toasts, and a tap on the shoulder when you're elsewhere.** New
  notifications (a mention, a DM, a share, an approval) now surface as
  in-app toasts on every surface: a small stack bottom-right, click to go
  there, six and a half seconds and gone. A watcher in the app shell rides
  the same liveness the Home feed uses (the SSE firehose plus the 30s poll
  as the floor), so a mention that lands while you're on Boards reaches you
  there. The first read is the baseline: reloading never replays the inbox
  as a burst of toasts, and past four stacked the oldest quietly goes. The
  browser half is deliberate: an OS notification fires ONLY when Talaria is
  not what you're looking at (another tab, another window, minimized) —
  while you're looking at it, the in-app toast already said it. Permission
  is asked from your click, never on load: Settings → Notifications gains a
  Desktop notifications row (On/Off, with the blocked case explained), and
  a local off switch stands in for the grant browsers won't let a page hand
  back. Two open tabs each see the arrival; the OS shows it once (the
  notification id doubles as the OS tag). This covers the open-but-
  background case, which the plain Notification API does from a tab;
  notifying a fully closed browser is Web Push and stays unmade. Verified
  live: a row landing while focused toasts with no OS notification, the
  toast click routes to its target, the same arrival with focus elsewhere
  fires the OS notification with the right tag (headless Chromium denies
  real notifications and reports every page focused, so that leg ran
  against browser stubs plus unit tests of the gate), and a reload toasts
  nothing. 11 new tests; full suite green (2580).

- **The app updates itself, from inside the app.** Admin → Security carries an
  Updates panel: it shows the running commit, checks the remote, and an
  Update now button that pulls the latest release, installs, builds into a
  staging directory and swaps it in, then restarts the server with no shell
  involved. Manual by default; an Update automatically toggle (off until
  someone turns it on) has the scheduled check install updates on its own
  every few hours. The restart is the careful part: the build lands in
  `ui/dist-next` and is swapped for `dist` only when complete (a live server
  never serves a half-built bundle), the old `dist` stays behind one
  generation as a manual rollback, and a detached helper waits for the port,
  boots the replacement, and waits for it to answer. The new server is the
  one that marks the update done, and an update that never lands shows as
  exactly that rather than a fake success. Safety rails: a dirty checkout on
  the server is refused rather than pulled over, dev installs never update
  (vite reloads on its own), and `TALARIA_UPDATER=off` is the kill switch for
  deployments supervised some other way. Verified live: the panel's API
  answers in dev with mode dev and refuses to apply there, a check against
  the real remote reports behind/ahead honestly, the auto-update toggle
  round-trips and persists, and 9 unit tests cover mode gating, dirty-tree
  refusal, and the running → done/failed reconciliation across a restart.
- **Bun is the repo's runner, dev through production.** A root `package.json`
  is now the hub for every way to drive the repo — `bun run dev` (full
  stack), `build`, `start` (production server on the Bun runtime), `test`,
  `typecheck`, `check` (invariants), `verify` (all of it) — and both packages
  carry `bun.lock` (migrated from their npm lockfiles; versions unchanged).
  setup/dev install and build through bun, the MCP server runs its TypeScript
  entry directly in dev (`bun src/index.ts`; tsx dropped) and production
  boots with `bun server-entry.js`. Node stays a hard floor — vite, vitest,
  svelte-check, and tsc run under it — so CI installs both, pinned to bun
  1.4.0. Verified live: `bun run verify` green from the root (2559 tests, 0
  type errors, invariants pass), a full MCP stdio handshake under bun, and a
  production boot serving real API traffic against live Postgres/Redis with a
  clean SIGTERM shutdown (the scheduler's 9 jobs drained).
- **Agents can clean up an inbox, without ever being able to empty one.** Three
  new fleet tools over the connected Google account: `list_labels` (Gmail's
  folders ARE labels — INBOX and UNREAD are system ones), `create_label`
  (find-or-create, so a retry is safe), and `organize_emails` (apply/remove
  label names on up to 100 messages by id: removing INBOX archives — mail stays
  in All Mail — removing UNREAD marks read). The HITL line is deliberate and
  follows the platform's own rule: sends and invites leave the building under
  the owner's identity and wait for approval, while filing, archiving and
  mark-read stay inside the mailbox and are reversible — so they apply
  immediately, because "clean up my inbox" behind fifty approval cards is not
  cleanup. TRASH and SPAM are refused everywhere (service layer, sandbox,
  agent-facing routes), so nothing in the toolkit can delete mail. Message
  listings now carry label names alongside each message. Organizing needs the
  `gmail.modify` scope (swapped in for `gmail.readonly`); a connection granted
  before this needs one reconnect, and the routes say so when they hit it.
  Fixtures grade the two real risks: filing into a label that was named but
  never created, and reorganizing a mailbox without reading a single message in
  it — including archiving an unread mail its owner still needs.
- **Your Google Workspace account, connected to your assistant where you look
  at your assistant.** Settings → Assistant now carries the connect card:
  Gmail · Calendar · Drive, with the safety story beside the button — reads
  are live, and every email or invite the assistant drafts waits in your
  Inbox for your approval before anything sends. The Connections tab says the
  same thing instead of "Google Drive & Docs", the connect callback lands on
  the tab that interprets its result (it used to land on Profile, where the
  outcome flash never rendered), and the Inbox assistant panel offers a
  one-link "Connect Google" in its footer — right where a "can't reach your
  mail" reply actually bites.
- **The assistant can actually read the mail it manages.** Two new fleet
  tools: `read_email` (one full message by id — headers plus the complete
  plain-text body, decoded from Gmail's nested MIME tree, capped at 20k
  characters) because the listing tool only ever returned snippets, and
  `search_drive` (find files by name in the Drive the agent acts for, with
  links — read-only). Both are modelled and simulated in the fitness toolbox,
  same as every tool in the kit, and both refuse a legacy shared-key caller
  the way the existing Google tools do.
- **Google approvals show the payload, not a paraphrase.** An agent-drafted
  email or calendar event surfaces in the Inbox as a P0 approval card whose
  recommendation says "review the exact outbound payload" — which the card
  never showed. The evidence rows are now the payload's own fields: To,
  Subject, and Body for an email; When, Where/who, and Notes for an event —
  so the human approves what will actually go out, not a summary of it.

### Changed
- **The app talks like a teammate now.** Every piece of user-facing copy was
  swept: "command your fleet" language is gone (the login line is "Sign in
  and get to work.", the brand tagline is "get things done together"), the
  word "fleet" retired from anything a person reads in favor of your agents
  and the team, em dashes came out of copy (~430 across components, routes,
  and the server strings the UI shows, replaced with plain punctuation:
  two sentences, a colon, a comma), and the stiffest sentences were
  shortened and loosened. What was deliberately kept: the em dash as a
  "no value" glyph in tables, en dashes in ranges, LLM prompt prose, and
  code comments. Verified: svelte-check 0 errors, the full suite green
  (2569 tests), and a residual scan shows no em dashes or fleet words left
  in rendered copy.
- **Docs match the platform again.** Three read-only audits diffed the doc set
  against the code; what they caught is fixed: `ui/README` no longer claims a
  React/TanStack Start stack or a violet-to-magenta Mercury accent (both
  pre-Svelte, pre-gold) and says bun where it printed npm, `CONTRIBUTING` and
  `HANDOFF` match it (typecheck is svelte-check, not `npx tsc`), `mcp/README`'s
  install and example config use bun, `MCP.md` names the rendered gateway URL
  for what it is (the app's own port, not the toolkit's standalone listener),
  `AGENT-NETWORKING` gains a dated update on what the preflight covers now
  (the app port, the `/api/mcp/gw` path agents actually dial, external DNS)
  and the chassis's pinned resolvers, `WORKBENCH` explains why the built-in
  browser depends on that DNS, and per-person timezones (brief open time,
  digest arrival) are documented for the first time. Historical entries —
  dated plans, the changelog itself, the phase log — were left as history.

### Removed
- **The golden-image (VM) deploy is retired.** `scripts/image/` — the Proxmox
  build/bootstrap/firstboot scripts and their systemd units — and
  `docs/SELF-HOSTING.md`, which documented only them, are deleted. The
  container deploy (docs/CONTAINER.md) is the self-hosting path: same app,
  same env-var contract, a compose file instead of a VM template, and it
  doesn't need a Proxmox host to provision one. References repointed
  (README self-hosting pointer, doc index, CONTAINER.md intro).

- **Fleet is gone from Home.** The Fleet tab (admin-only) and its pulse view
  are deleted — Agents shows the same running state, and Observability owns
  the deep view, so a third copy was overhead nobody normal asked for. The
  home summary no longer computes fleet health (one fewer container status
  pass on every load), and the assistant's surface map no longer knows a
  Fleet destination. Old links keep working: `/fleet` redirects to /agents,
  and `/home/fleet` falls back to the Inbox like any unknown tab. Verified
  live: `/api/home` returns no `fleet` key, the Home tab strip renders
  Boards/Comms/Plans/Research/Docs with no Fleet entry for an admin, and the
  surface tests (including the `/home/fleet` fallback pin) pass.

### Fixed
- **Agent helpdesk tickets file to a board everyone can actually see.**
  `report_problem` used to land on a personal helpdesk board only the
  reporting user could open — a helpdesk nobody staffs. Problems now file to
  an org-wide Helpdesk board under the Talaria team: org-wide boards
  materialize an editor membership for every user (at ensure-time and on
  every sign-in), so the whole org sees the board the moment something files
  to it. Verified live: a synthetic agent problem posted with a real
  per-agent key found the board org-wide with all four users holding editor
  seats, and the ticket renders on /boards under Talaria → Helpdesk.
- **The default admin can still sign in when Google owns the login screen.**
  With a Google client connected the login screen is now the clean
  one-button experience it should be — Continue with Google centered, no
  password card — and the fallback is a quiet "Admin sign-in" whisper in the
  corner that unfolds a password card (Escape closes it). Verified live in
  both themes: Google button centered, no centered password form, the corner
  disclosure opens and closes, and the admin signs in through it.
- **Daily briefs file under Agents → [agent] → Briefs, not My Files.** Brief
  transcripts used to drop into the owner's root folder where nothing else
  lives. The artifact writer now ensures the per-agent Briefs folder chain,
  and a migration (two appended statements — the second catches mirrors whose
  folder the code path had already built; the checksum ledger refused the
  in-place edit exactly as designed) moves every existing mirror into place.
  Verified live: brief mirrors sit under Agents → Gregosaurus → Briefs.
- **The dithered selector on segmented controls no longer washes out its
  label.** The quiet band's ink weight is halved (0.78 → 0.5): the selection
  still reads unmistakably against the tile, but the label reads through it.
  Verified live on /agents — the highlighted option's label is clearly
  legible over the texture.
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
- **The assistant panel answers with its tools now, steered by the view.** The
  sidebar conversation used to open every detached turn with "Tools are
  disabled" — disarming the owner's personal assistant made every live-state
  question ("how many tickets are on Finance?") unanswerable except by
  invention. The reply harness now arms the persona's own governed tool loop,
  and the prompt tells it which tools to reach for FIRST: the ones that match
  the view the panel is floating over (Boards → list_boards/list_tickets/
  get_ticket/comment; Knowledge → search_knowledge and the KB reads; Comms →
  channels and messaging; …), then its other tools when those cannot answer.
  The surface tool lists are server-side, keyed by the same id the briefs use,
  so a client cannot write tool names into the prompt. Inbox queue-card
  sign-offs keep their own path: propose through the command branch, confirm
  with a click — never the detached conversation. Three inbox-reply fixtures
  were reworded to the new contract (unbacked action claims still fail;
  Inbox-card action ids still fail; live-state answers must be grounded in
  tools or hedged, never invented).
- **The assistant panel follows the conversation again.** Two scroll fixes:
  sending now jumps to your message (a reader parked up in history used to
  stay parked while the reply streamed below the fold), and opening or
  expanding the panel starts at the NEWEST turn — the transcript remounts at
  scrollTop 0 on every expand, which used to open on the oldest of what
  loaded.
- **The assistant panel's composer rail matches the platform.** Attach sits
  left, and the effort chip + send/stop tile are right-aligned (the rail had
  no spacer, so everything packed against the left edge). The footer's
  standing "No tools" readout is gone — it now says "Tools on", which since
  the tools change above is the truth.
- **The assistant panel no longer runs past the app height when docked.** In
  flow mode (≥1400px) the panel's aside turned `relative` with no height of
  its own, and a block child of the collapse pane is auto-height — so the
  flex-1 transcript stopped scrolling and grew to its full content height,
  pushing the composer below the bottom of the app. The aside now carries the
  nav rail's own methodology (`h-full` inside the `h-full` CollapsePane), so
  header, scrolling transcript, and composer always compose to exactly the
  pane's height. Overlay mode (below 1400px) was already definite
  (`absolute inset-y-0`) and is unchanged.
- **The effort picker now appears on deployments upgraded past its ship.** The
  stored per-model catalog's only production writer is the model-adder modal,
  so a catalog written before the effort extraction had no levels for anyone
  and the chip stayed hidden until an admin re-opened the modal. An empty read
  on `/api/models/efforts` now runs a one-time backfill — the serving
  endpoints' catalogs are refreshed live (once per endpoint; a catalog written
  by the current build never re-triggers, and a failed refresh retries no more
  than every five minutes) — and the route answers from the fresh store. Also:
  the agent chip is gone from the chat composer rails (Comms, Plan, Research) —
  the sidebar owns the conversation partner, and the rail's right side is now
  tier, effort, then the send/stop tile.
- **Model cost autodetect is consistent now.** The price oracle only ever
  priced an endpoint's REGISTERED models — but tier routing and aliases
  attribute usage to models nobody registered (grok via a tier mention,
  gemini "-latest" aliases), which stayed unpriced forever, and id shapes
  like "~"-prefixed aliases, ":free" variants, "-latest", and trailing
  release dates never matched OpenRouter's catalog ids. The oracle now
  prices the union of registered models and every model usage has
  actually landed on (keyed by the exact usage string so the costing
  join hits), matches through alias fallbacks (strip "~", ":variant",
  "-YYYYMMDD", "-latest", vendor-prefixed suffix match), and an unpriced
  cloud usage row nudges a refresh ahead of the 6h cadence (15min
  throttle). Live result: unpriced cloud tokens 13.5M → 0.

### Added
- **The Google OAuth client is registered in the Admin UI, not a .env file.**
  Admin → Org → Google Workspace · OAuth client: paste the client ID, secret,
  and an optional Workspace domain restriction; the secret is sealed
  (AES-256-GCM) in the database and never shown again. The panel lists the
  exact redirect URIs to authorize in Google Cloud Console — account connect,
  org connect, and login — with copy buttons, which was the step the .env
  workflow never helped with. The env vars remain as a fallback for scripted
  deployments; the Admin-UI record wins when one exists, and removing it hands
  control back to the env. Google LOGIN stays env-flag-gated
  (AUTH_GOOGLE_ENABLED) on purpose — registering a workspace client must not
  silently open a new way into the instance — but the flag now works with
  credentials from either source.
- **A platform-default reasoning effort, yours.** Settings → Preferred model
  now offers a "Default reasoning effort" pick directly beneath the model —
  but only when the selected model publishes effort levels (a model with no
  ladder shows no control; there is nothing to default). The saved level
  becomes the starting pick everywhere effort is offered — Comms agent chats
  (tiers included) and the assistant panel — wherever the model in play
  supports it; models that don't simply run at their own default and show no
  chip. An explicit pick in any conversation (including auto) stays
  authoritative for that conversation, and the default re-seeds exactly when a
  model switch retires the pick. Stored on the profile beside the preferred
  model (`users.preferred_effort`); picking auto clears it. Because the
  preference travels across models, the server stores the bare level string
  and each surface applies it only against the levels that model's metadata
  vouches for — a stale level is inert, never an error.
- **Agents carry their own default effort, set where the model is picked.**
  The agent editor's model rows — main model and every tier alias — now show
  the effort chip beside the picker when the chosen model publishes levels,
  and the saved level becomes that agent's conversation default: Comms DMs
  with the agent (or the tier) and the assistant panel start there, and the
  chat routes apply it server-side when a sender made no pick, re-validated
  against the model's live levels so a stale config is inert. Precedence is
  specific-over-general: your explicit conversation pick > the agent's
  configured default > your platform default (Settings) > the model's own.
  `/api/models/efforts` answers both halves (`efforts` + `default`), and the
  persona resolver reads the configured effort from the same cached agent
  config walk that resolves capability keys — one read, one TTL, re-pointing
  an agent's model or effort follows within a minute.
- **The Comms composer is one honest tile.** The gold send tile now becomes
  the stop square while a reply streams (ChatComposer's `onStop`) instead of
  stop appearing beside submit, the "⏎ send" key-hint chip is gone — the
  component itself deleted, its three remaining uses (comms rail, the channel
  composer, the research start bar) removed with it — and the effort picker
  was rebuilt in the agent-chip anatomy: strong-border mono chip with the
  3×12 bar meter marking where the pick sits on the model's ladder, opening
  the §7 popover with a bar meter on every row.
- **Sending while a reply streams no longer interrupts it — anywhere in the
  turn.** The server queue (`queue: true` + the chained follow-up turn) has
  always existed, but a message sent during the FIRST turn's opening window —
  after the request leaves, before the response headers carry the
  conversation id back, which the server can hold for minutes behind a
  restarting agent — took the fresh-send path and started a second stream,
  forking the thread. Those messages are now held locally and flushed the
  moment the id lands (or re-sent as a fresh turn if the first one died
  without producing one); a hold never leaks across a thread switch, and
  attachments stay live while streaming because the queue carries them. Stop
  now means stop, too: the turn freezes what was on screen instead of the
  live-resume poller re-animating the server's copy until it finished anyway.
  Verified: `npx tsc --noEmit`, `npm run typecheck` (0 errors), `npm test`
  (2476 passing), plus the queue paths walked by hand in the running app.
- **Reasoning effort reaches the primary chat surfaces.** Comms agent DMs and
  the assistant panel now offer an effort chip on the composer rail, right
  aligned just left of the send tile — but only when the model's own catalog
  metadata vouches for levels. OpenRouter-style catalogs publish
  `reasoning.supported_efforts` per model; that list is extracted into the
  stored per-model catalog metadata at the same moment an admin adds models on
  /models (the live-catalog refresh the model adder already triggers), so the
  picker lists exactly the levels the provider says the model accepts and
  nothing else. A model that publishes no levels gets no chip, and its requests
  carry no `reasoning_effort` at all. Server-side, `/api/chat` and the inbox
  focus command validate the pick against the same metadata (a persona id is
  resolved to the model actually serving it, tiers included), send it as
  `reasoning_effort` on the outbound turn, and honor the effort of a QUEUED
  message on the chained turn that covers it (stamped on the message row,
  re-validated before the chain runs). Tool-offering gateway turns keep the
  `'none'` workaround — function tools and an effort cannot share a request.
  Verified: `npx tsc --noEmit`, `npm test` (2476 passing, including new
  coverage for the catalog extraction, the pool intersection, the persona
  resolution, and the transport bodies), `npm run typecheck`, plus a live
  catalog read against OpenRouter confirming every `supported_efforts` list
  arrives on a model that also advertises the `reasoning_effort` parameter.
- **Mercury turns Talaria into a focused operator workspace.** A unified
  near-black visual system now carries the full authenticated product, while
  Inbox becomes a risk-ranked decision queue with source evidence, guarded
  actions, persistent history, and an adjustable assistant panel that follows
  the operator across work surfaces — addressing the owner's own assistant by
  the name they gave it. The spec-matched composer keeps attachments,
  agent and model selection, response modes, MCP access, skills, help, and
  dictation in one conversation flow without weakening existing approvals.
- **Platform hardening: one API dialect, a modular UI kit, a complete SDK.**
  A two-sided audit (all 162 API routes; the whole component tree) worked
  through to zero: a guard module (`requireUser`/`requireAdmin`/
  `requirePerm`/`requireView`/`parseBody`/`actorOf`) replaced ~160
  hand-rolled auth prologues and ~90 body validations; 400s now name the
  first zod issue; audit logging landed on every sensitive mutation that
  lacked it (agent secrets, endpoint key rotations, gateway keys, agent
  edits, cron lifecycle, KB creates, ACL grants); a provider key moved
  out of a GET query string; org-wide views (cost, fleet, inference)
  gained real authz. The kit grew Tabs, Checkbox/Toggle, SectionHeader,
  Segmented, DropdownMenu, SaveButton, CopyButton, Chip modes, and
  EmptyState variants — then the eight worst offender surfaces were swept
  onto them at visual parity. The SDK now exports every primitive and
  type a third-party app needs. Config writes are PUT; actions are POST.
- **Talaria is an app platform.** Apps are self-contained codebases in
  `apps/<slug>/` that compile INTO the deployment and load as native
  surfaces — work views, manage views, settings tabs — on the same
  router, design system, and session as core. Each app can ship its own
  API (`/api/apps/<slug>/*` with an authenticated user handed to every
  handler), a migrations-free per-app document store, and MCP tools for
  agents that register in the MCP registry (badged "app") under the SAME
  granular governance — per-agent assignment, tool subsets, per-person
  access, gateway-enforced, dispatched in-process. Apps are
  explicit-grant: enabling one gives members nothing until an admin
  allows its views per person. Manage → Apps has Installed + Discover:
  a marketplace catalog (community + official, configurable index) and
  install-from-any-git-URL — a shallow clone into `apps/` IS the
  install; dev picks it up live, prod flags "awaiting build". Built on
  `@talaria/sdk` (docs/SDK.md); `apps/contacts` is the reference.
- **Invites + transactional email.** The third admission door: invite an
  email address, they get a branded join link (public /join page shows
  who invited them, to which org, bound to which address), and signing in
  with Google admits them, stamping the invite accepted. 14-day expiry,
  one live invite per address, instant revoke, state chips. Email rides a
  provider seam — your own SMTP (Google Workspace works with an app
  password) or Resend — with sealed secrets, masked GETs, and a
  send-me-a-test button. Invite creation survives a broken mail config
  (the error surfaces; the invite persists).
- **Sign-up domains + the instance's own address.** Two kinds of domain,
  deliberately separate. EMAIL sign-up domains (Admin → Org): prove
  ownership via a DNS TXT record at `_talaria-verify.<domain>` and anyone
  with a matching address may self-join — verification is mandatory, so
  nobody claims gmail.com. The HOSTING domain verifies by a self-fetch
  round trip: the server requests its own identity beacon through the
  candidate domain and checks the instance id that answers — proof DNS,
  routing, and TLS land on THIS deployment. Once verified it is the
  canonical base URL for MCP OAuth callbacks and links.
- **Knowledge goes Notion-lite.** Quote-anchored COMMENT THREADS with
  in-text highlights and resolve; multiplayer presence (who's here,
  view/edit); Muse IN the page — an always-on chat bar that edits the doc,
  plus select-any-text → Comment or Muse for surgical rewrites; context
  menus across reading and editing (with full table controls and a
  visible table toolbar); image paste/upload; inline artifact embeds; a
  clean Official promote/demote lifecycle (double-confirm demote) with
  agent-doc governance; and per-doc OKF summaries (GoogleCloudPlatform
  knowledge-catalog spec) — an agent-facing frontmatter+summary the
  Librarian maintains autonomously as official docs change. Space pages
  and doc pages now share one editor grammar.
- **MCP OAuth 2.1, end to end.** Servers that answer 401 get the full
  spec treatment: protected-resource metadata → RFC 8414 authorization-
  server metadata (path-aware, so github.com/login/oauth resolves) →
  dynamic client registration → PKCE, with resource indicators. No-DCR
  providers (GitHub) get a manual OAuth-app flow: Talaria shows the
  callback URL to copy and links the provider's own app portal from its
  service_documentation. Tokens seal per subject (org or user), refresh
  silently, and force a visible reconnect on revocation. Registry changes
  that alter what a running agent carries roll the fleet blue/green —
  Hermes wires MCP at process start, so a render alone was never enough.
- **Org-wide MCP with a real marketplace.** A registry of MCP servers
  governed per agent AND per person (assignments ∩ allowances, tool
  subsets everywhere, "All tools" explicit), enforced at a gateway that
  filters tools/list and rejects tools/call — config can't be jailbroken
  past the registry. The add flow searches the official MCP registry
  (latest schema) with tier ranking so real remote servers beat wrapper
  noise, brand icons, a featured business shelf, credential-driven
  install forms from declared headers, and publisher resolution for
  names the registry lacks (well-known manifests, documented endpoints).
  Per-user servers connect each person's own account under Settings →
  Connections. The built-in Talaria toolkit became a governable registry
  row too — same subsets, same gateway, identity locked.
- **Fine-grained permissions.** A 13-permission catalog across agents,
  work surfaces, knowledge, artifacts, files, templates, and models —
  resolved three layers deep (per-user overrides → org member defaults →
  shipped defaults), admins unconditional, enforced server-side by the
  same helpers everywhere. Admin → People shows per-person chips with
  override provenance, one checklist gating work views, manage views,
  and (later) app views, and agent allow-lists beside them.
- **Comms goes Slack-lite: threads, reactions, paste-a-file, edit &
  delete.** Channel messages (channels, Relays, DMs) now spawn THREADS —
  a side panel with the root + replies and its own composer; replies stay
  out of the main flow, roots carry a "N replies · when" rollup, and an
  agent @mentioned inside a thread answers IN the thread with the
  thread's own conversation as its context (replying to a reply re-roots
  Slack-style — threads never nest). REACTIONS: a quick-react palette on
  hover, chips with counts and who-reacted tooltips, click to toggle —
  and agents can react under their own identity (one of our twists).
  FILES: paste an image or drop files straight into any composer
  (channels and agent DMs) — they upload immediately and ride as
  attachment chips, images rendering inline. Plus own-message EDIT
  (inline, enter-to-save, "(edited)" marker) and DELETE (author or
  channel owner; a root takes its thread with it, the confirm says so).
  Relay conclusions summarize thread content too. The composer itself is
  now the Slack-shaped RICH EDITOR: markdown syntax as you type (**bold**,
  `code`, ``` blocks, > quotes, lists) with a formatting toolbar in a
  controls row under the full-width input, Enter sends (inside a code
  block it newlines), Shift+Enter soft-breaks, @mention and :emoji:
  autocomplete inline, and a searchable emoji picker next to attach.
  Reaction palette dismisses on Esc/outside-click/mouse-leave.
- **Platform sub-agents (Models → Platform).** Talaria's own workers are
  now first-class, named agents — separate from the Hermes fleet, each a
  model-agnostic harness with its own skills for one internal job: Muse
  (prompt-editing everywhere), Distiller (chat → private-brain distills),
  Concluder (relay closing summaries), Catalog writer (model blurbs),
  Judge (ticket outcome review), and Briefer (view briefings; fixed to
  your personal assistant by design). Each agent's model is configured
  granularly on the new Models → Platform tab — an admin pick wins while
  it routes, otherwise the job's auto chain keeps working untouched (the
  Judge's pick shares judge_config with the Guard panel, one source of
  truth). All platform work is now metered under `platform:<agent>`
  callers, so spend per sub-agent is attributable.
- **Dynamic titles (the Titler).** A seventh platform agent names things
  as they take shape: chats and plans are retitled after their first real
  exchange — but only while the title is still the mechanical truncated
  first message, so a name a user typed is never clobbered — and research
  runs get a concise title from their question the moment they start
  (shown across the run list, header, and briefings; the raw question
  remains underneath). Naming is fire-and-forget: a rate-limited or dead
  model keeps the current title, never blocks the work. A RETROACTIVE
  hourly sweep (kicked from comms reads, mirroring the distill sweep)
  names everything that predates the Titler or whose call failed —
  batched per pass, fail-fast when the model is down — so old chats,
  plans, and research runs pick up titles too. Chats can also be renamed
  by hand from the thread context menu (owner or plan collaborator);
  a hand-picked name is never overwritten.
- **Personal agents get a private RAG brain.** Every user's personal
  collection ("My knowledge") is now the assistant's long-term memory of
  that user, created lazily and bound to the owner + their assistant only:
  chat distills land there (alongside the owner-scoped ambient copy —
  search merges dedupe), and personal research reports index there instead
  of sitting unreachable in the activity index. A personal assistant now
  retrieves as its **owner's proxy** — the owner's channels, boards, plans,
  and distilled history, exactly what the owner could retrieve and nothing
  more (org agents keep their board-policy scope). Org-wide research
  reports get an `orgWide` payload + matching scope clause, so they're
  finally retrievable at all. Also fixes agent-key RAG search 502ing on a
  uuid cast in the team-binding clause.
- **Personal-content privacy pass + multiplayer research.** Personal-agent
  output is now private to its owner everywhere: PA-created documents, KB
  docs, research reports, and generated media carry the owner's
  `owner_user_id` and default to `private` (org agents keep publishing
  org-wide). Research is scoped like plans: you see your own runs, runs
  shared with you, and org-wide (agent-initiated) runs — and it's now
  **multiplayer**: a `research_members` table, `/api/research/:id/members`
  (share by email, owner-only, with notification + automatic editor grant
  on the report artifact; collaborators can leave), and an avatar-stack
  share UI in the run header mirroring plan sharing. Briefings only
  surface research you own or were invited to.

### Fixed
- **ACL audit fixes — six leaks closed.** (1) `/api/uploads/:id` streamed
  any file by id; now gated by `canAccessUpload` — owner, admin, or
  reachable through a conversation/channel/board/artifact the viewer can
  actually read (agents resolve through their board/channel grants or
  their owner's chats). (2) `/api/history` served full snapshot bodies for
  any key; now enforces per-kind ACLs (artifact + KB perms incl. space
  inheritance and editor grants, memory/skill by agent ownership,
  templates admin-only). (3) `/api/memory/:id` GET let any user read any
  agent's memory; now admin-or-owner like PUT. (4) KB search matched raw
  row visibility, ignoring space inheritance and grants — a doc in a
  private space leaked into results; now filters through the same
  effective-permission check the read routes use. (5) Artifact link
  DELETE had no read gate (POST did). (6) Research list/get had no owner
  predicate at all — everyone saw everyone's runs.
- **Templates view (Manage → Templates).** Templates managed in one place
  instead of a modal buried in board settings: tabbed by consumer
  (Tickets · Plans, with counts), deep-linked down to the selected
  template (`/templates?tab=plan&t=…`), context menus, list + detail
  layout with rendered skeleton preview, blur-saving name, and prompt-only
  agent guidance behind tooltips. The body edits in the full workspace
  editor — rich editing, Muse prompt-drafting, and NEW version history
  (template bodies snapshot on save with author attribution, served
  through /api/history like souls and skills). The old template library
  modal is deleted; board settings links out. **Muse is harnessed for
  skeletons**: hard structural rules (## sections only, 3–6 of them,
  placeholder hints or empty stubs — never content, under 25 lines, and a
  request for a finished document still returns only the skeleton) —
  adversarially verified live: a "14-step process in full detail" ask and
  a "fully written project plan" bait both came back as ~17-line
  skeletons.
- **Context menus, platform-wide.** Right-click is real now: one primitive
  (`ui/context-menu.tsx` — cursor-positioned, keyboard-navigable, portaled,
  viewport-clamped) wired onto every row, card, and tile: kanban cards and
  board list rows (open / copy link / copy ticket ref / archive), nav board
  rows, channel/relay/DM rows (open / copy link / mark-read that reads the
  REAL cursor), agent thread rows, message bubbles (copy text), knowledge
  spaces and docs, artifacts and folders (incl. copy PUBLIC link when a
  slug exists), the agents roster (the full lifecycle cluster, confirm
  texts shared with the buttons so they can't drift), research runs, and
  every home console row. Every item calls the exact function its button
  counterpart calls — a context menu is a shortcut, never the only home of
  an action (the rule lives in UI-CONVENTIONS). Native browser menus are
  suppressed app-wide EXCEPT on editable fields, where paste and
  spellcheck are real workflows.

### Changed
- **Navigation cleanup.** The sidebar is work surfaces only: the bottom
  logo is gone, the System section is gone — Settings and Admin live under
  the user menu (Admin still hard-gated by role independent of what the
  rail renders). **Compute, Cost, Audit, and Alerts merged into one
  Observability view** (`/observability`): the root is an Overview —
  alerts strip worst-news-first, generating-now + fleet temperature,
  gateway pulse, spend today, recent audit trail, each clicking through
  to its detail tab (`?tab=compute|cost|audit|alerts`). The four old
  routes are DELETED, not redirected — every reference (nav, home fleet
  tiles, server-side notification hrefs, the activity indexer) was
  rewritten to the new structure. **Models is tabbed**: Models (provider
  registry) · Roles · Access, deep-linked, with the verbose panel
  descriptions and field explanations moved into ⓘ tooltips.

### Added
- **The Inbox is a console.** Home rebuilt as one tab per work area — Inbox ·
  Boards · Comms · Plans · Research · Docs · Fleet (admin-only) — each
  pairing the area's live state with its recent activity, badges marking
  where attention is needed, quick-action cards gone. Boards gets a queue
  SIDEBAR (triage / review / blocked) with the FULL list per queue; audit
  trails (board/channel activity) and the notifications feed are collapsed
  by default. Failed work is a first-class signal: a plan whose last turn
  errored and an errored research run both show "failed — open to
  re-run/retry →" in their tabs and deep-link straight to the spot.
- **Assistant briefings, per view.** Your personal assistant opens each
  console tab with a short read on what needs you — at most five bullets,
  each SCOPE with its own prompt (delivery-lead framing on Boards,
  "who's waiting on a reply" on Comms, "what moved" on Plans,
  "ready-and-unread reports first" on Research). Briefings cover ONLY
  unreviewed things (unread, pending, in-flight, failed) and regenerate
  only when that set actually changes — fingerprinted on ids and counts,
  never on a timer. Deliberately EPHEMERAL: the summary lives in one
  replaceable row, and chatting back (right in the panel) streams through
  the assistant with the briefing as context, persisting nothing — no
  conversation rows, nothing indexed, nothing for comms-decay to distill.
- **Deep links everywhere.** The URL is now the selection across the
  platform: `/?tab=…`, `/knowledge?space=…&doc=…`, `/artifacts?a=…`,
  `/research?r=…`, `/comms?c=…`/`?a=…&x=…`, `/plan?p=…`, `/admin?tab=…`,
  `/settings?tab=…`. The old one-shot apply-then-clear pattern (which made
  nothing copy-linkable) is gone: picks are push navigations so
  back/forward walks your trail, defaults/healing are replace. Rule
  recorded in UI-CONVENTIONS → Deep links.

### Fixed
- **Skeletons were invisible.** The entire skeleton system (and tiptap
  table borders, and the grey status dots) painted with `var(--theme-line)`
  — a variable that never existed as raw CSS. One root alias fixes every
  shimmer at once.
- **The inbox took ~5 seconds; every RAG call stalled 3.6s.** The embedding
  client's configured docker-internal hostname fails DNS slowly before
  falling back — and the fallback was never remembered, so EVERY embed
  (search, indexing, health probes) paid the stall. The resolved base is
  now sticky. On top: `computeAlerts` ran its eight probes serially (now
  one Promise.all + a 15s cache; 4.3s → 7ms) and three surfaces shelled
  out to `docker ps` in the same breath (now a 5s cache). Home: 4.8s →
  ~40ms warm; KB search: 36ms.
- **Loading-state audit, platform-wide (~70 gaps, 35 files).** Every
  fetch-backed component now holds a layout-matched skeleton while in
  flight; EmptyStates render only after a query RESOLVES empty. Parent
  queries no longer block unrelated siblings (plan stage, models panels,
  boards' serial fetch, comms message pane). Real bugs flushed out along
  the way: clicking an agent before conversations resolved silently
  started a NEW thread instead of resuming the working one; the memory
  quick-add could clobber the file if used mid-load; a fleet-wide cron
  created while the roster loaded would target nobody; false states
  ("Not connected", "API keys are not enabled", unchecked policy
  checkboxes) flashed during every load.

### Changed
- **Dynamic greeting** (time-of-day pools, stable per hour), subtitle
  removed; notifications collapsed behind an unread badge so the
  assistant briefing is the working surface.
- **UI detail pass (live walkthrough, 2026-07-28).** A guided sweep of the
  whole surface, in nine moves. **Typography, two voices**: IBM Plex Sans
  for everything a person reads or types (markdown surfaces, editors, all
  form controls, board cards/lists, home rows, both content browsers); mono
  stays the chrome/identifier voice — the rule lives in UI-CONVENTIONS.
  **Dialogs take over the screen** (`Modal takeover`): content-heavy
  dialogs fill the viewport minus a gutter, constant height, content
  scrolls inside; deep editors (skill/memory/soul/config-history) open as
  ticket-style nested slide-ins INSIDE their dialog instead of stacking
  modals, with Esc peeling one layer at a time. **Agent editing reworked**:
  the skill editor no longer opens empty (it seeded from a still-loading
  fetch), memory gains quick-add + rich display, and crons got a real
  scheduling UI — pick daily/weekdays/weekly/monthly/interval with time
  pickers, see it in plain English, cron syntax generated underneath — plus
  in-place job EDITING via the never-wired `hermes cron edit`. **Retired
  agents can be deleted** (admin, confirm-gated): def + versions + secrets
  + rendered files + created-agent volumes; produced history and imported
  legacy volumes are kept. **Admin is tabbed by concern** (Organization /
  People / Agents / Retrieval / Storage / Security) and **Settings too**
  (Profile / Assistant / Connections / API keys) — with every panel's
  explanatory paragraph moved into ⓘ `InfoTip` tooltips. **The assistant
  is fully editable in Settings**: identity, handle, personality, model
  tier chips, start/stop, and inline Schedules/Skills/Memory — the
  user-friendly cut of the admin config, all owner-scoped. **Buttons never
  wrap** (`whitespace-nowrap` in the base). **Skeleton loading
  everywhere**: `Skeleton`/`SkeletonRows`/`SkeletonCard` replace blank
  panes and "Loading" strings across home, boards, tickets, agents, comms,
  browsers, cost, inference, activity, alerts, research — and the app
  shell itself now server-renders a skeleton frame instead of a blank
  page, which was the real "no content, then BAM" (the whole app was
  gated behind the client-side session fetch). Conventions for all of it
  are recorded in docs/UI-CONVENTIONS.md.

### Added
- **@mention autocomplete in the rich editor.** The thread #60 left open:
  ticket comments and descriptions NOTIFIED on mentions but their TipTap
  composers had no autocomplete — you had to guess the token. `RichEditor`
  now takes a `mentions` prop (a TipTap Suggestion popup, modeled on the
  slash-command menu): type `@` and pick from the people the mention will
  actually notify. Picks insert plain `@token ` text — the exact grammar
  the server parses and the renderer highlights, so the markdown
  round-trip is untouched. Wired on the ticket surfaces with board
  members: the comment composer and both description editors (inline +
  slide-in). Also closed for consistency: a ticket CREATED with an
  @mention in its description now notifies board members (only the edit
  path did; template-seeded descriptions can't fire it). KB/artifact
  bodies stay unwired on purpose — mentions there don't notify yet
  (eligibility undecided, recorded in TODO).
- **Universal @mentions (#60).** Coverage holes closed so a mention behaves
  the same no matter who writes it or where. Agent-authored mentions now
  notify: an agent posting to a channel (the agent-key write path returned
  before the notify call — "@jon can you check this" from an agent reached
  nobody), a streamed agent channel reply, and an agent's plan turn all
  notify @mentioned humans exactly like human messages. Ticket
  DESCRIPTIONS notify board members on change, mirroring the comment
  contract. Plan mentions got three fixes: the composer now suggests the
  plan's actual members instead of the whole org (mentioning someone who
  couldn't read the plan silently notified nobody), the notification
  deep-links to `/plan?p=…` instead of `/artifacts`, and a plan whose doc
  doesn't exist yet falls back to plan membership as the read boundary.
  And mentions are finally VISIBLE: a shared remark pass in the Markdown
  renderer highlights @tokens on every surface at once (chat, channels,
  comments, descriptions, plan turns) — code spans untouched, emails don't
  match. Still recorded as future threads: the TipTap @-suggestion (rich
  composers), research/KB mention eligibility.

### Changed
- **WYSIWYG everywhere (#46).** The remaining prose fields that still edited
  as raw textareas now use the shared rich editor: template skeleton bodies
  (the sections agents fill — now edited the way tickets render them),
  the create-agent soul draft (parity with the post-creation editor),
  assistant personality in both the wizard and the inline settings field,
  and the plan modal's AI-drafted ticket descriptions. One long-standing
  inconsistency fixed: YOUR chat bubbles now render markdown like every
  other message surface (they were the one bubble showing raw text). The
  plan document gains the fullscreen toggle (Esc exits) that artifacts and
  KB docs already had. Deliberate leaves, now written into
  docs/UI-CONVENTIONS.md → Editors: chat/channel composers stay plain
  textareas (Enter-to-send + caret mentions beat a toolbar; porting
  mentions to a TipTap suggestion is its own thread), and machine/prompt
  text (YAML, raw HTML, AI instructions, template guidance) stays mono
  Textarea on purpose.

### Added
- **Inference: live dashboard + container controls (#48).** The Compute page
  is now a real inference dashboard: a live strip showing **generating right
  now** (per agent, from streaming reply rows with a 10-minute recency clamp
  and partial indexes to keep the scan tiny), the **gateway pulse** (upstream
  calls / errors / time-to-first-byte p50+p95 over the last 15 minutes,
  in-memory — a pulse, not a ledger), **fleet temperature**
  (up/warming/unhealthy/down), and the last hour of per-agent activity; the
  page leans into a 5s poll while anything is generating or warming. The
  fleet roster finally shows the **warm-up state**: Docker's `starting`
  health phase (the healthcheck's 60s start_period) renders as a pulsing
  amber "warming up" dot instead of masquerading as up, and `up`/`unretire`
  return immediately instead of blocking the request for up to two minutes —
  the polled roster tells the story. Two new controls per agent:
  **Restart** (quick bounce, confirm-gated since in-flight replies drop;
  owners can restart their own assistant) and **Roll** (admin — the existing
  zero-downtime blue/green replacement, previously only reachable through
  config saves, now a button: fresh container warms up while the old one
  keeps serving, then drains). Both audit-logged. Verified live: restart
  showed `starting` → healthy on the roster, a roll flipped the slot with
  the old container serving throughout, and a real streamed reply appeared
  in "Generating now" with the gateway pulse moving. (Managing inference
  BACKEND containers — Ollama/vLLM lifecycle — is the ROADMAP's separate,
  bigger thread; this ships the agent-container half plus the dashboard.)
- **Proactive agent outreach (#59).** Agents stop waiting to be asked, two
  ways. **`message_user`** — the first agent→human write path: a governed MCP
  tool that starts (or continues) a real chat conversation with a teammate
  plus an inbox notification deep-linked to it (`/comms?a=…&x=…`). Personal
  assistants can only reach their owner; every agent↔person pair is
  rate-capped per day (the declined-send reason goes back to the agent so it
  can adapt). **The check-in sweep** — opt-in (master switch off by default
  AND per-agent flags, Admin → Proactive outreach): each opted-in agent
  periodically gets an automated turn through its OWN persona gateway showing
  its stale/blocked/waiting work and its recent outreach ("don't repeat
  yourself"), and acts through its normal tools — ticket comment, channel
  post, or message_user — so everything stays attributed, board-policy-gated,
  guard-visible, and ledger-metered. `outreach_events` logs every check-in
  and DM (caps, repeat-avoidance, admin visibility). Verified live: with
  nothing stale Jax replied NOTHING_TO_SURFACE; given a 96h-blocked
  high-priority ticket he chose to DM the board owner through message_user —
  message in the conversation, notification in the inbox, reasoning in the
  log — and the dedupe context correctly suppressed a near-duplicate
  follow-up; owner-only + daily-cap + unknown-target guards all refuse with
  plain-language reasons.

### Changed
- **Input consistency sweep (#49).** Every form control now speaks the same
  keyboard language, via two tiny shared helpers (`submitOnEnter`,
  `inlineEditKeys` in `ui/control.ts`) instead of hand-rolled `e.key`
  handlers: inline title/name edits (KB space + doc, artifact, ticket) commit
  on Enter and revert on Escape — Escape is shielded from modal/document
  handlers so cancelling an edit no longer risks closing the surface;
  field-plus-button rows submit on Enter (provider API key, assistant handle
  rename, agent secrets, MCP server add, rerank key, audit retention,
  federate directory, version note); the home-dashboard email compose — the
  one dialog still hand-rolling its own backdrop and raw inputs — now sits on
  the shared Modal (Escape/backdrop close for free) with the shared field
  primitives and focuses "To" on open; and the login username, provider-create
  name, and save-image title fields focus on open. Audited ~80 text controls
  across 33 files; 16 gaps fixed, the rest already followed the conventions.

### Added
- **Hermes self-review — agents review their own work before the QA judge
  does (#78).** Three moves. First, the plumbing bug that blocked all of it:
  Hermes only discovers skills outside its home via `skills.external_dirs`
  in config.yaml, and Talaria never rendered that key — the `/opt/skills` +
  `/opt/dept-skills` mounts existed in every container but the skill registry
  never scanned them. Rendered configs now carry both roots, so every
  Talaria-managed skill is a first-class enabled Hermes skill (live: `hermes
  skills list` shows them as `local/enabled` fleet-wide). Second,
  `subagent-driven-development` (official Hermes registry, MIT,
  obra/superpowers-derived) is vendored into `scripts/skills/` and seeds to
  every install — with the bundled `requesting-code-review` it gives agents
  fresh-context implementer/reviewer subagents per task. Third, the reflex:
  the talaria-toolkit skill and every rendered soul now teach "before
  `report_outcome`, self-review against the ticket's requirements —
  `requesting-code-review` for code, `subagent-driven-development` for
  multi-task plans" — the QA judge becomes the second line, not the first.
  Bonus: shared-skill seeding upgraded from copy-if-missing to pristine
  tracking (`fleet/skills/.seeds.json`) — copies still byte-identical to what
  was seeded follow canonical updates; admin-edited copies are never
  clobbered (both paths verified live).
- **Explicit plan-template picker.** Plans could only seed their living document
  from the agent's bound plan template, implicitly — the missing half of the
  chain tickets already had. The Plan surface now shows a Template picker (in
  the header, for a new plan before its first turn) listing every plan-kind
  template; the pick rides through plan creation onto the conversation
  (`conversations.plan_template_id`) and becomes the highest link in
  `resolveTemplate` — explicit pick → agent binding → none. "Automatic" leaves
  it to the agent default. The chosen skeleton seeds the doc verbatim and
  shapes every later agent rewrite. Verified live: a plan created with an
  explicit template seeds its doc from that skeleton, automatic falls through
  to the agent binding, and the pick persists on the conversation, 7/7 checks.
- **Confab guard: PII check + coaching.** A fifth check, `pii_leak`, catches
  high-precision personal data in model output — SSNs, Luhn-validated payment
  card numbers, IBANs (emails/phones stay unpoliced: they're everyday
  workspace content) — and strict mode now redacts PII alongside secrets in
  whatever Talaria persists or hasn't yet relayed. And the guard finally
  closes its loop: an opt-in **"Coach agents from findings"** toggle turns
  repeated findings (≥2 of a check in 7 days) into templated behavioral notes
  in the agent's rendered soul — per-check counts + fixed advice only, so
  flagged CONTENT still never re-enters any model's context; delivery is at
  render time, a performance review between sessions rather than a mid-turn
  correction. Verified live: a raw model echoing a test SSN through the
  public route came back `[redacted SSN]` with a caveat and an out-of-band
  finding, and rendered souls gain/lose the coaching block as the toggle
  flips, 8/8 checks.
- **Attachments reach the model everywhere.** Two asymmetries closed: channel
  replies now hand image attachments to agents as data-URL image blocks
  (1:1 chat already did — group channels were text-only), and TEXTUAL file
  uploads (markdown, csv, json, code, ...) now contribute their contents to
  the prompt in both paths — send, queued-turn history rebuilds, and channel
  transcripts — clipped like ref chips. File bytes re-read only for the
  recent transcript tail; images capped per reply. Verified live with a real
  agent: quotes a token from an attached file in a DM and in a channel, and
  perceives an attached image identically in both paths.
- **Toolkit onboarding — agents get the playbook, not just the tools.** The
  talaria MCP was attached to every agent but nothing taught them when to
  reach for it. Now a fleet-wide `talaria-toolkit` skill (seeded from
  `scripts/skills/` on render, admin-editable after, mounted read-only at
  `/opt/skills` — a mount that was documented but never actually wired) walks
  the reflexes: search before planning, keep the ticket alive, durable output
  goes in Talaria, drafts await approval, report_problem on breakage. The
  rendered SOUL header points at it.
- **`fetch_attachment` toolkit tool.** Agents can now READ the files attached
  to tickets and chats: text formats come back inline (clipped at 50k chars),
  images arrive as real MCP image blocks the model can see, and other binary
  formats report honest metadata instead of pretending. `get_ticket` now
  advertises the attachments array. Verified live: fleet render seeds skill +
  mount, MCP serves the tool, text/image/binary/404 behaviors all correct,
  11/11 checks.
- **Object storage — built-in bucket, bring-your-own, or both.** Upload blobs
  can now live in a real S3-compatible bucket instead of local disk, three
  ways: the **built-in bucket** — a bundled MinIO container (dev-compose
  `minio` service, creds via `TALARIA_S3_*` env, bucket auto-created) so you
  get durable object storage with no cloud account; any **external**
  S3-compatible service (AWS S3, Backblaze B2, Cloudflare R2, MinIO) via
  endpoint/bucket/keys in Admin → Storage (secret sealed by secretbox); and an
  optional **replica** that mirrors every new upload to a second provider as
  it lands (fire-and-forget — a replica outage never blocks an upload), with a
  "Sync all" that backfills everything already stored and automatic read
  fallback to the mirror when the primary can't serve a blob. The client is a
  hand-rolled SigV4 signer over fetch — no SDK. Each upload's row records
  where ITS bytes live (`s3+internal://` / `s3://` / filesystem path), so
  switching modes never strands a file. Connection tests do a real
  write/read/delete round-trip; a background migration moves local files into
  the active bucket. Verified live 22/22 across two runs: external-bucket flow
  against a throwaway MinIO, then built-in mode with auto-created bucket,
  replica mirror-on-upload, full sync, and replica fallback after deleting the
  primary object.
- **Ticket attachments.** Tickets now carry the same attachment chips as chat
  messages: uploaded files plus knowledge-doc/artifact refs (ACL-checked
  against the attacher, content clipped into the chip for models). Attach and
  remove from the ticket detail; changes log to the ticket's activity. Agents
  see attachment metadata in `GET /api/tasks/:id` and can now pull the bytes
  from `/api/uploads/:id` with the fleet key; agent callers can attach uploads
  but not refs (no session to ACL-check). Verified live end to end, 11/11
  checks.
- **Hybrid retrieval — keyword and meaning, fused.** Every brain now indexes
  each chunk twice: the dense embedding it always had, plus a sparse
  bag-of-terms vector (Qdrant IDF-modified, so exact identifiers like env
  vars, ticket numbers, model names, and error strings survive whole).
  Searches fuse both branches with reciprocal-rank fusion, so
  `TALARIA_EMBED_MODEL` finds the doc that names it AND "how do embeddings
  get configured" finds it too. Legacy dense-only brains keep working
  untouched until rebuilt.
- **Guided reindex — the repair path for a changed embedding model.** Swapping
  `TALARIA_EMBED_MODEL` changes vector dimensions and silently breaks every
  index/search against the old collections. Talaria now probes what the
  embedding service is actually serving (model + dimension, shown in Admin →
  Retrieval) against the LIVE Qdrant collection shape — never the registry,
  which had already gone stale once — and raises a critical alert plus an
  admin banner when they diverge (or when a brain predates hybrid search).
  One "Rebuild index" button recreates each brain in the current model's
  shape and refills it from the workspace's own records; index-don't-copy
  makes the rebuild lossless. Verified live: legacy 384d dense brains
  rebuilt to hybrid, exact-identifier and paraphrase queries both rank the
  seeded doc first, and stale points from deleted sources and
  pre-officialization grounding rules washed out in the process.
- **Confab guard: annotate and strict modes now act.** They were configurable
  but every path discarded the result — observe was effectively the only mode.
  Annotate pins findings to the flagged reply (`messages.guard` /
  `channel_messages.guard`) and renders a warning caveat under it in chat and
  channels (channels update live via republish); the public LLM route appends
  the caveat to non-streaming responses and injects one final SSE delta before
  `[DONE]` on streams. Strict additionally redacts detected secrets (keys,
  tokens, whole private-key blocks) from whatever Talaria persists or hasn't
  yet relayed, so saved copies and future transcripts stay clean. Agent-loop
  keys (`gateway_unmetered_keys`) never receive caveats — a finding must never
  re-enter a model's context; internal utility completions (judge, muse,
  research) likewise stay observe-only so parsed outputs can't be corrupted.
  Admin copy now describes what each mode actually does.

## Phase 7 — self-contained under Talaria (2026-07-09)

Everything routes through Talaria, on one network, with no Dockerfiles and
secrets encrypted at rest.

### Changed
- **Fleet routes through Talaria's gateway.** Every model spec in each rendered
  agent config is rewritten to point at Talaria's gateway (`/api/llm/v1`);
  Talaria routes each model to the provider you register on `/models`. Agents
  have exactly one upstream. Legacy litellm model names are bridged to real
  provider ids (`glm → z-ai/glm-5.2`).
- **One `talaria` docker network** for every Talaria container (dropped the
  legacy `ai_default`). The self-hosted inference server is just a registered
  provider, reached like any other.
- **Bridge eliminated.** The app reaches each agent's persona gateway directly
  on a stable published loopback port (`fleet/fleet.json` = model → url + key);
  `proxyChat`/`listAgents` read the manifest. Removed `bridge/`, `ui/Dockerfile`,
  and the legacy build-based `stack/` — **no Dockerfiles** remain; the run path
  is compose-only (official/published images) + host-run app.

### Fixed
- **Knowledge search finds space overviews.** Top-level spaces are documents
  too — search now sweeps their name + overview body alongside docs, and a
  space hit opens the space itself.
- **Research uses the same agent selector as every other surface** — the
  standard picker at the top of the rail (like Plan), not a bespoke composer
  pill. The composer keeps only the depth pill.
### Fixed
- **Plan mode plans; it no longer files tickets.** Plan-surface turns went
  to the agent with its full toolkit and no hint it was in a planning
  session, so an eager agent would create real tickets mid-conversation.
  Every plan turn (live and server-chained) now carries a plan-mode
  harness: think and decide with the teammate, read anything, create
  NOTHING; tickets come from the Draft tickets control once the plan is
  settled. Verified live with an explicit "create the tickets" bait: zero
  mutating tool calls, and the agent pointed to Draft tickets instead.
- **The plan document now actually builds as you talk.** The side-by-side
  doc only ever updated when someone clicked "Sync from chat", despite the
  empty pane promising otherwise, so new plans looked broken: you talked,
  the doc stayed blank. Every landed agent turn now triggers the sync
  automatically (unsaved manual edits are flushed first so a rewrite starts
  from them), with the rewriting overlay as feedback; the manual button
  stays for on-demand refreshes. Verified live: a fresh plan filled its
  document from the first exchange with no clicks.

### Changed
- **Agents converse like colleagues.** Every rendered soul now carries a
  voice contract: acknowledge in a sentence (or ask ONE clarifying question
  when the ask is ambiguous) before diving into tools, do the full work but
  keep the process out of the chat, and report outcomes like a busy human:
  what happened, where it lives, judgment calls worth flagging. Em dashes:
  most replies need zero. Verified live: the same agent whose replies were
  walls of process narration answered in five sentences with a sensible
  scoping question. Souls hot-reload, so this took effect fleet-wide with no
  restart.
- **Em dashes swept from the platform's own copy.** Around 120 gratuitous em
  dashes across 35 files of visible UI copy (placeholders, tooltips, empty
  states, hints) rewritten with ordinary punctuation. The dash survives only
  where it means something: empty-value glyphs in tables and the brand
  tagline. Codified in docs/UI-CONVENTIONS.md.

### Fixed
- **Queued chat messages get their reply on screen.** Sending while an agent
  was still replying queued correctly server-side, and the follow-up turn was
  generated — but the chat never showed it (it only appeared after a reload).
  The chat now keeps watching whenever the last visible message is the
  user's, so the server-chained reply streams in on its own. Verified live
  with a queued mid-stream message.
- **Agents can edit the docs they authored.** Editing a KB doc the agent
  itself created returned 403 (edits required an explicit editor grant, and
  create_kb_doc granted nothing) — so agents worked around it by creating
  duplicates. Authorship now grants edit; everyone else's docs still need a
  grant.

### Added
- **Agents can open a knowledge space — create_kb_space.** "Put this in a
  new Company space" no longer needs a human errand first: agents can create
  a space (find-or-create by name, so retries never duplicate), while
  sharing, deletion, and marking docs official stay human calls.
- **Agents reach for Talaria first — flailing fixed at three layers.** A real
  transcript showed an agent burning 20 tool calls hunting for Notion/
  Obsidian/vaults when the answer was one toolkit call away. Now: (1) every
  rendered soul carries a **toolkit contract** — the talaria MCP is the first
  reach for anything workspace-shaped, with the tool names spelled out and a
  hard "there is no Notion/Obsidian/Airtable" line; (2) the Hermes image's
  **conflicting bundled skill packs** (note-taking/obsidian, productivity's
  notion + airtable + google-workspace, the ungoverned email pack) are pruned
  on every roll and fresh boot — surgically, everything else stays; (3) the
  gaps that CAUSED improvisation are closed: **create_kb_doc** (the "add to
  knowledge base" job was literally impossible), **list_teammates** (resolve
  a name to an email for drafts/board shares), **list_board_members**,
  **list_research**, and a folder param on create_document. Verified live:
  the same task that flailed now completes in two calls.
- **Agents fail gracefully — report_problem.** When something breaks on the
  agent's side, it no longer dumps endpoints and error internals on
  non-technical teammates: the new tool alerts every admin, files a
  **Helpdesk** ticket with the technical details (board find-or-created),
  and hands the agent plain-language reassurance to relay. The soul contract
  teaches the etiquette; a new critical alert fires when the fleet's MCP
  endpoint itself is unreachable (the root cause behind "connection refused"
  flailing).
- **Attach anything — knowledge, artifacts, or files.** The composer paperclip
  is now a menu: attach a knowledge doc or an artifact (search pickers) or
  upload a file. Knowledge/artifact picks become reference chips on the
  message — the referenced content travels to the model on that turn and on
  every later history rebuild (queued turns, resumes, channel transcripts),
  ACL-checked against the attacher, with truly-private items silently
  undiscoverable. Chips render in history and link back to their source.
### Changed
- **Chats are keyboard-first — the send button is gone.** Enter sends
  everywhere (Esc stops a streaming reply); the enlarged "⏎ send" / "esc
  stop" key chips beside the input are the affordance, fading in when live.
  Composer controls all sit on one optical line, with depth/agent/tier pills
  hugging the input's right edge.
- **UI consistency pass — one app, not fifteen.** A full audit (screenshots +
  code inventory, docs/UI-CONVENTIONS.md is the contract) and the first big
  unification: shared surface primitives (`RailSurface`/`Rail`/`Stage`/
  `StageHeader`/`RailRow`/`CountPill`, `IconButton`, `Chip`/`StatusDot`/
  `DangerLink`). Plan's sidebar moved to the LEFT like every other surface and
  gained a real header; Research, Comms, Knowledge, and Artifacts all sit on
  the same w-72 rail with the same h-12 header line running straight across.
  Fewer, smaller controls: contextual actions are icon buttons with tooltips
  (no more giant "+ New space"/"+ New" primaries), Send/Go became one icon
  affordance (Enter submits everywhere), the reranker panel autosaves on
  change (only the API key keeps an explicit save), destructive actions are
  quiet red links instead of buttons, and ellipsis is gone from all UI copy.
### Added
- **Brain routing everywhere content lives.** The same "Brain" control now
  sits in the top menu of BOTH knowledge docs and artifacts (docs, sheets,
  microsites): Auto / a specific brain / None. For artifacts, explicit
  assignment indexes the rendered content (sheets become tables) into that
  brain only — plan documents and research reports routed away from Auto
  leave the activity brain, and flipping back restores them. Content edits
  re-index in whatever home the routing says. Owner-only on both surfaces;
  privacy still trumps routing.
- **Per-doc brain routing — brains contain only what's assigned to them.**
  Every KB doc gains a "Brain" control (owner-only, next to Official): **Auto**
  (space binding / official→org rules), a **specific custom brain**, or
  **None** (never indexed). Explicit assignment always wins, re-placement is
  immediate, and privacy still trumps routing — a private doc only ever
  reaches its owner's personal brain. Members can see brain names for the
  picker; the binding matrix stays admin-only. **OpenRouter** joined the
  reranker registry (US) — and it reuses the LLM endpoint key you already
  registered, so reranking needs zero extra setup.
- **The RAG stack lives — and got a real retrieval pipeline.** The retrieval
  plane (Qdrant + a native CPU embedding model via TEI, default
  `bge-small-en-v1.5`) is now part of the self-contained compose — it had been
  silently dead since the Phase-7 stack cut, with every index call swallowed.
  Three defenses so that can't recur: a critical **alert** when either service
  is unreachable, a one-click **"Reindex everything" backfill** (Admin →
  Retrieval; content-hash idempotent) that restored the workspace's history,
  and a **15-minute incremental sweep** that self-heals missed rows after an
  outage. Retrieval gained a **reranker** precision stage: a provider registry
  like LLM endpoints — self-hosted TEI, Voyage AI (US), Together AI (US),
  NVIDIA (US), Pinecone (US), Cohere (Canada), Jina (Germany) — with live
  model catalogs where providers expose one, sealed API keys, and graceful
  fallback to vector order (reranking can never break search). And RAG brains
  are finally **curatable in the UI**: create a brain, bind who can search it
  (teams now supported alongside users/agents/everyone), and point KB spaces
  at it — every non-private doc in a bound space feeds that brain instead of
  the org default, re-routing immediately on bind/unbind.
- **Artifacts file themselves — every agent gets a cabinet.** Auto-created
  artifacts stop piling up at the root: each agent gets a folder named after
  it, with category subfolders created on demand — **Plans** (plan documents),
  **Research** (reports), **Documents** (agent-authored docs via MCP),
  **Media** (image saves without an explicit folder), and **Chat summaries**.
  Distilled idle chats now ALSO become browsable artifacts (private to the
  chat's owner) instead of living only in the activity brain. Filing is
  best-effort by construction — a folder hiccup can never kill the flow
  creating the artifact. Humans' hand-created artifacts and explicit folder
  picks are untouched.
- **Research view (#56) — Perplexity-grade cited research, run by YOUR agents.**
  Ask a question on `/research`, pick a depth — **Recon** (one fast pass, a
  cited answer), **Brief** (planned angles, a briefing document), **Expedition**
  (iterative deep dive with gap-chasing rounds, a full report) — and whose
  expertise should drive it. The pipeline runs server-side, outside any chat
  context: the chosen agent's own persona plans the queries, judges the gaps,
  and writes the document, while Perplexity sonar models (through the org
  gateway, metered) run the search stages and supply sources. Every factual
  claim carries an inline [n] citation against a deduped global source
  registry; unresolvable markers are stripped and a mechanical Sources section
  is appended. Reports are org-visible doc artifacts (versioned, shareable,
  exportable), indexed into the activity brain, with a completion notification
  deep-linking back (`/research?r=`). Every agent gets `research` +
  `research_status` MCP tools — an agent researches its own field without its
  conversation window ever swallowing a search dump.
- **Model Roles — tailor the model stack per activity.** `/models` gains a
  "Model roles" panel: assign which model handles each class of work — a
  **search model per research tier** (Recon / Brief / Expedition — Perplexity's
  sonar family maps one-to-one; pointing a tier at a deep-research-class model
  makes the engine run fewer, bigger stages instead of multiplying effort) and
  **Utility** (catalog blurbs, chat distills, Muse fallback) are live;
  **Image understanding**, **Image generation**, **Embeddings**, and
  **Reranker** slots are reserved for their surfaces. Unset = auto (sensible
  pick from what's registered); an assignment only wins while it still routes,
  so a deleted model can never silently break a subsystem. Admin-only, audited.
- **Multiplayer Plan — several humans, one plan.** Plans are no longer
  owner-private: the owner shares a plan by email (avatars + share control in
  the plan header), and collaborators get the whole surface — the conversation
  (they talk to the same agent; turns carry author names so voices stay
  distinct, in the UI and in the agent's transcript), the living document
  (auto editor grant, revoked on leave), ticket drafting, and agent Sync. A
  "Shared with you" sidebar section surfaces plans riding other agents;
  sharing notifies with a `/plan?p=` deep link; presence rings show who's
  viewing right now. Owner shares/removes; collaborators can leave. The doc
  stays owned by the plan's owner, and @mention notifications now reach every
  collaborator (they're doc readers by construction).
- **QA judge scores against the ticket template.** At quality review the judge
  now resolves the same template chain ticket creation uses (assignee binding →
  board default) and receives the skeleton as an objective rubric: every
  section must be meaningfully addressed ("n/a" only where truly inapplicable);
  missing or skeleton sections come back as named "revise" issues.
- **Brain-routability health — unroutable agents surface instead of freezing.**
  Provider pools churn under no-train routing; when an agent's configured
  model drops off its endpoint, chats used to hang silently. Every enabled
  agent's config targets (main / tiers / fallbacks) are now probed against the
  gateway registry (30s cache): an unroutable MAIN raises a critical alert
  ("X's brain is unroutable") and a red chip on the agent's card; dead
  tiers/fallbacks get a warning + amber chip. Fix from /models or the agent.
- **Comms follow-through: unread badges, DM notifications, thread peek.**
  Per-member read cursors (`channel_members.last_read_seq`) drive unread
  count pills on every channel/relay/DM row; having a channel open marks it
  read live. A DM message now drops an inbox notification outright (deduped:
  while one sits unread, further messages fold into it), deep-linking back to
  the conversation via `/comms?c=<id>`. Agent rows in the sidebar gained an
  expand chevron so you can peek at an agent's threads without selecting it.
- **Elevated assistants — promote an admin's assistant to org-wide view/edit.**
  Admin → Users gains an "elevated assistant" toggle on admin rows
  (`agent_defs.elevated`). An elevated personal assistant reaches every live
  board (tickets + governance as editor), every channel and relay, and gets
  implicit editor rights on org-visible knowledge docs and artifacts. Hard
  lines that elevation never crosses: human↔human DMs, other users' private
  items, owner-only actions (board team moves, deletes, sharing changes).
  Elevation is only effective while the owner is an admin — demote the human
  and the assistant's reach collapses with them (demotion also clears the
  flag). Audited (`user.assistant_elevated`).
- **Drag boards between teams.** In the nav rail, a board's owner can drag it
  onto another team group (or Personal) to move it — groups highlight as drop
  targets, empty groups say "drop here". Server-side the move is owner-only
  (it changes who can see the board): `PATCH /api/boards/:id { teamId }`, or
  `{ teamName }` by name ("personal" clears the team).
- **Personal assistants can join group channels — behind a privacy gate.** The
  hard block is gone: your assistant can be added to a shared channel (still
  only by YOU — someone else's assistant never shows up in your picker). Its
  group replies carry a privacy gate above channel instructions: never reveal
  the owner's private context (memory, mail, calendar, private docs) outside a
  DM with the owner, and never use owner-identity tools on a channel's behalf —
  it declines and points people at the owner.
- **Ask your assistant to run your boards.** A personal assistant now acts as
  its owner for board governance (`actingUser` identity proxy): move a board
  between teams, share/unshare it by email, and allow/remove agents. Five new
  MCP tools — `list_teams`, `move_board_to_team`, `add_board_member`,
  `remove_board_member`, `set_board_agents` — assistant-only by construction
  (general agents get 401; the routes resolve identity server-side, and team
  moves still require the owner role). `list_boards` now shows a personal
  assistant its owner's boards (with the owner's role) alongside its
  policy-allowed boards, and `GET /api/teams` answers to the identity proxy.

### Changed
- **Inbox is tailored to you AND the org.** Two zones: the personal column
  (notifications, approvals, your triage/review/blocked queues, agenda, mail,
  quick cards, assistant) beside an org rail titled with the business name —
  fleet health, a live activity **Pulse** across boards/comms/fleet, and for
  admins two glance tiles: live alert count and today's spend (both deep-link).
  Members see the pulse without the admin numbers.
- **Home and Inbox merged.** `/` is now **Inbox** — the top nav item and the
  landing surface: notifications up top (mark-read on open, mark-all-read, the
  panel disappears when quiet), then the day's dashboard (assistant, approvals,
  agenda, mail, triage/review/blocked queues, fleet glance). The unread badge
  moved to the top-level item; `/inbox` redirects; quick cards point at the
  current surfaces (Comms · Plan · Boards · Artifacts).

### Added
- **The Talaria toolkit is ATTACHED — every agent has its tools.** talaria-mcp
  grew a fleet HTTP mode (stateless streamable-HTTP, per-request identity via
  X-Agent-Name, fleet-key auth); the app self-hosts it as a supervised child
  (probe-guarded, respawning) and every rendered config now carries the
  `talaria` MCP entry automatically. Agents get the whole safe surface —
  tickets, artifacts, channels, KB, `save_image_artifact` — on their next
  roll. Closes the long-standing "HTTP transport for containerized agents"
  backlog item (#58).
- **Agents speak product, not plumbing.** The org soul header now instructs
  agents to point teammates at workspace surfaces (Artifacts, boards, docs)
  instead of file paths and containers, unless the person is working at that
  technical level.
- **Save agent images to Artifacts.** Every agent-produced image in chat gets
  a hover "Save to Artifacts" (title + folder picker) that copies it out of
  the agent's container into a durable file artifact. Agents can do it
  themselves too: the talaria MCP grew `save_image_artifact` (path + title +
  folder-by-name, find-or-create) — agent saves default org-visible so the
  team actually sees them. For science. And company meme folders.
- **Agents can show images in chat.** Files an agent creates in its own
  container and references as `MEDIA:<path>` render inline in DMs and
  channels, streamed through `/api/agent-media/:model` — gated on the same
  access as chatting with the agent, restricted to images under the agent's
  own `/opt/data` volume (traversal-proof, size-capped, nosniff), slot-aware.
  The rewrite happens at render time, so past messages light up too; remote
  image URLs in replies already rendered via markdown.
- **Send while the agent is replying.** Agent chats flow like Claude: messages
  sent mid-reply queue into history without interrupting, and when the current
  reply finishes the server automatically runs the next turn covering
  everything queued (chaining until the conversation goes quiet, surviving
  reloads — the follow-up turn is server-driven). The composer stays live
  during streaming (Stop and Send side by side); dead streams can't wedge a
  conversation (10-minute staleness guard). Applies to agent DMs and Plan
  chats; channels were already non-blocking.
- **Org-voice model blurbs.** Model descriptions get ONE rewrite pass into
  task-oriented one-liners ("what it's good at, when to pick it") in the org's
  voice, cached in `model_blurbs`; newly registered models get theirs on the
  next catalog read (throttled, detached). Raw public-catalog text is the
  fallback; nothing is invented for unknown models.
- **Learned parameter support at the gateway.** When an upstream 400 names a
  parameter we sent (newer models retire tunables — sonnet-5 rejects
  `temperature`), the gateway strips it, retries, and remembers per
  endpoint+model so later calls pre-strip. Dynamic specs straight from the
  provider — no tables to maintain.
- **Member model access + human-friendly model picking.** Admins choose which
  models non-admins may pick for AI drafting / preferred model (Models →
  Member access; empty = all, admins never restricted), enforced server-side
  in the catalog, the preference save, and muse resolution (a restricted
  preference falls back). The picker itself grew up: models show a pretty name
  and a one-line "what it's good at" blurb, populated automatically from the
  public catalog (no maintained lists; unknown/self-hosted models simply show
  their id).
- **Rolling agent replacement — edits never kill a conversation.** Each managed
  agent runs in one of two compose slots; applying a change brings the incoming
  slot up on a **fresh port** beside the old container, cuts the manifest over
  only after real health (the app re-reads it per call, so traffic shifts
  instantly), drains in-flight replies (`TALARIA_ROLL_DRAIN_SECONDS`, default
  45), then retires the old container. A newcomer that never gets healthy is
  discarded — the old agent never blinks. Org saves and config/MCP applies roll
  instead of restarting; `proxyChat` additionally holds-and-retries through any
  residual gap instead of failing (or answering with the mock).
- **Organization config — agents join YOUR team.** Admin → Organization sets
  the business name + what it does. Woven in automatically everywhere agent
  identity forms: muse-generated agents/souls/personalities anchor to the
  business, and every rendered SOUL.md opens with an org header (a render-time
  projection — stored souls stay clean, existing agents pick it up on the next
  render/restart). Agents stop introducing themselves as "on the Hermes team."
- **Comms — every conversation in one place.** Chat and Channels merge into a
  single Slack-shaped, agent-native surface (`/comms`): persistent **#channels**
  (ambient talk), **Relays** (named ad-hoc gatherings of people + agents around
  a purpose), **teammate DMs** (human↔human, riding the channel machinery,
  deduped per pair), and **agent DMs** (durable 1:1 threads). One sidebar, four
  sections; old `/chat` and `/channels` routes redirect.
- **Conversations decay instead of accumulating.** Relays **conclude**: a
  summary of what was decided is posted as the final message, indexed for
  retrieval (channel-membership ACL), and the relay archives. Idle agent DMs
  (default 14 days, `TALARIA_CHAT_TTL_DAYS`) are **distilled** — durable
  substance summarized into the activity brain, owner-scoped — then archived
  out of the sidebar. Sweeps run opportunistically (throttled hourly, never
  blocking a request); plans are exempt (they're documents, not scrollback).
- **Ticket & plan templates.** An org-wide template library (markdown skeleton
  + agent guidance per template — the headings are the schema): boards bind the
  ticket templates they use and mark a default (Board settings → General);
  agents can carry overrides (agent modal → Summary → Templates). Resolution
  everywhere: explicit pick → agent binding → board default → freeform. Applied
  when agents draft tickets from plans/channels, when the plan document is
  created/synced, and at ticket creation itself — a bare ticket (quick-add or
  an agent's create tool) is seeded with the resolved skeleton.
- **Dependency-aware ticket drafting.** Planners propose `dependsOn` ordering
  between drafted tickets; the review modal shows/edits them as "blocked by"
  chips, and creation wires real ticket dependencies. The review modal itself
  is board-first (the board's template shapes drafts), roomier (wide layout,
  full description editing), and numbered for dependency reference.
- **Generation-in-progress states.** A shared `Generating` treatment (shimmer
  skeleton lines + stepped dots, plus an in-place overlay variant) replaces
  button-label-only waits: drafting tickets shows skeleton proposal cards,
  the plan document veils while the agent rewrites it, cron drafting shows a
  designing row, and ticket creation counts down.
- **Plan view, phase 2 — the document lives.** "Sync from chat" has the plan's
  own agent rewrite the living plan document from the conversation so far
  (`POST /api/plan/:id/doc`, metered like any chat turn; agent preamble and
  code fences are stripped). Draft tickets now treats the plan document as the
  curated source of truth, with the transcript as supporting context.
- **@mentions on the plan surface (#60).** The plan composer autocompletes
  teammates (shared mention machinery extracted from channels into
  `components/chat/mentions.tsx`); mentioned users are notified once they can
  read the plan's document (owner-private plans mention silently until shared).
- **Plans feed the activity brain (#63).** Plan turns and the living plan
  document are indexed into the ambient activity collection, ACL-scoped to the
  plan's owner (`planOwnerId`) — private planning never surfaces for anyone
  else. Hand edits to the doc re-index via the artifact save path.

### Fixed
- **Fresh-install model selection.** Bare model ids that contain `/`
  (OpenRouter-style, e.g. `qwen/qwen3-14b`) were mistaken for
  `endpoint/model` pins, leaving the preferred-model picker empty and the muse
  with "no models configured". The gateway catalog now tags qualified ids
  explicitly (`GatewayModel.qualified`).
- **No-train routing pool is fetched live.** The OpenRouter US no-train
  provider pool comes from `GET /providers` (US datacenters/HQ) on every call
  (briefly cached) instead of a hardcoded six-provider list that had gone stale
  and 404'd models it no longer served ("No allowed providers are available").
  A stored `only` list is only the offline fallback.
- **Provider catalogs are always live.** Preset seed model lists are gone —
  adding a provider drops straight into the endpoint's manage modal, where
  models come from the provider's live `/models` catalog: full list browseable
  on focus (the old picker capped at 8 alphabetical matches, hiding newer
  models), provider ordering preserved (OpenRouter lists newest first), and
  pagination followed (Anthropic pages at 20 by default).
- **Fleet network self-creates.** `fleetUp` ensures the external `talaria`
  docker network exists before `compose up` — a fresh install no longer fails
  with "network talaria declared as external, but could not be found"
  (`setup.sh` also created the wrong name, `talaria-fleet`).

### Security
- **Provider API keys encrypted in the DB**, not in configs. Sealed with
  AES-256-GCM in `llm_endpoints.api_key_cipher`; entered on `/models`, never
  returned to a client. Existing config keys are migrated into the DB
  automatically.
- **Envelope encryption + one-click rotation.** A random 256-bit DEK encrypts
  every secret and is stored wrapped by the root secret, so the
  unlock-everything key is never in a config. Admin → Encryption rotates the key
  and re-encrypts every secret (provider keys, agent secrets, OAuth tokens) in a
  single pass. All symmetric AES-256 — post-quantum-safe (no asymmetric crypto).

### Documentation

- **The README quick start covers both ways to run Talaria, with the commands
  for each.** Dev (`bun talaria setup` → `talaria dev`) and server
  (`bun talaria deploy up`, optional `bun talaria service install`) each get a
  copy-paste block that starts from the clone — which the old section omitted —
  under headings that make the choice explicit. Prerequisites now say plainly
  that Docker and Bun are hard dependencies and Podman is untested. The stale
  "prints your generated admin credentials" claim is corrected everywhere it
  lived (README, DEVELOPERS.md, docs/CLI.md, docs/CONTAINER.md, ui/README.md,
  the dev-loop skill, the entrypoint's header comment): there are no default
  credentials; a fresh instance comes up empty and the account you create at
  the claim screen is the admin — CONTAINER.md's dead `grep 'Sign in'` for the
  credentials that no longer print is replaced by `bun talaria deploy creds`.
  RELEASES/ left untouched as frozen history. Verified: `bun run check` green.

## [Unreleased]: Phase 6 — product depth (2026-07-06)

Turning the elegant shell into a capable product: one place to manage each
agent, attachments, personal assistants, governance, and a real audit trail.

### Added
- **Unified agent management modal**, one modal per agent with tabs — Summary ·
  Config · Skills · Memory · MCP · Versions. Every internal (previously separate
  top-level pages) lives here: config editing, skills (WYSIWYG + history),
  memory, MCP with live connection testing, and version history with one-click
  revert. Read-only for non-admins.
- **Agents roster redesign**, a toggleable **grid / list** where each agent
  shows only name, **role**, a health dot (up/degraded/down/retired/legacy from
  real container state), and icon controls (start/stop · manage · duplicate ·
  retire/migrate/re-hire). Detail moved into the modal.
- **Editable agent roles** (`agent_defs.role`) — a human title (e.g. "Support
  Lead") shown on the roster, set at creation, editable in the modal.
- **Re-hire + duplicate**, retired agents can be un-retired (re-enable → render →
  start from the preserved volume); any agent can be duplicated into a new one.
  Retire is a typed-slug double opt-in.
- **Fleet Reconcile**, one button renders all managed configs and starts every
  enabled agent that isn't running (drift + cold start; reboot survival is
  already handled by `restart: unless-stopped`).
- **Personal assistants**, everyone can spin up their own Hermes agent (own
  container, key, memory) from Home — `agent_defs.owner_user_id`,
  `createPersonalAgent()`, a "Your assistant" card.
- **Attachments in chat + channels**, images and documents (disk-backed
  `uploads` table, served from `/api/uploads/:id`); images render inline and are
  passed to vision models as data-URL content parts.
- **Per-view access control**, admins grant/revoke each primary view per member
  (`users.denied_views`); denied views are hidden from the nav and route-gated.
- **Audit trail + retention**, a real `audit_log` (actor · action · target ·
  before/after) wired into governance mutations, surfaced to admins on the Audit
  page; `audit_retention_days` is the first admin-editable app setting.
- **Chat tier picker**, the composer's raw model-tier `<select>` is now a
  premium portaled pill.

### Changed
- **Models page** is compact: provider cards show identity + a model count +
  Manage; the model list (with a proper catalog-search **add-model** flow, not a
  LabelPicker), pricing, class, and privacy routing moved into a modal.
- **Modals center on the viewport** (portaled to `<body>`) instead of within a
  backdrop-filtered card.

## [Unreleased]: Phase 5 — product IA + elegance (2026-07-06)

Reworking Talaria from a feature grid into a coherent product: two mental
modes, a real landing, versioned internals, and a self-hostable vocabulary.

### Added
- **Home / Today** at `/`, the seamless landing. In Talaria's guardrail model a
  person's job is to triage, review, and unblock the agents' work, so Home
  surfaces exactly those queues (scoped to your boards) plus unread mentions,
  quick entries into the work surfaces, and an accurate fleet-health glance
  (real container status). Chat moves to `/chat`.
- **Talaria LLM gateway** ([`server/llm-gateway.ts`]), one OpenAI-compatible
  endpoint over the whole model registry (`/api/llm/v1/{models,chat/completions}`,
  streaming). Provider keys stay server-side; per-endpoint `request_defaults`
  merge into every call; usage is metered per key. **Per-user API keys**
  (`tlk_…`, minted in Settings → API keys, admin-grantable via `can_mint_keys`).
- **No-train routing as a setting**, a per-cloud-endpoint toggle on /models
  (OpenRouter no-store allowlist + `data_collection: deny`, or portable deny) —
  privacy is opt-in, not baked in.
- **Versioned skills + memory**, every save is an immutable, authored revision
  (`internal_versions`); recover or load any prior one. Edited in a **WYSIWYG
  modal** (RichEditor + a history rail) — the raw textareas are gone.
- **MCP connection testing**, live status chips (Connected / Login required /
  Unreachable / Error) via a real MCP `initialize` probe carrying the agent's
  identity, plus a premium add-server modal that tests before saving.

### Changed
- **Navigation regrouped** into **Work** (Home · Chat · Channels · Boards ·
  Inbox) and **Manage** (Agents · Models · Compute · Cost · Audit · Alerts) +
  System — simple for the non-technical surfaces, control grouped for the
  technical ones. Skills/Memory/MCP/Models moved off the top nav into the Agents
  page. Fleet overview folded into Agents (`/fleet` → `/agents`).
- **"Local" → "Self-hosted"** in all user-facing copy (people run models on
  local machines *and* other on-prem boxes). "Local inference" → **Compute**,
  "Activity" → **Audit**.

### Removed
- The dead **`/tasks`** nav item (it matched the boards API route; no page ever
  existed behind it).

## [Unreleased]: Phase 2 UI (2026-07-02)

Talaria's own front end ([`ui/`](./ui), Vite + TanStack Start) grows a full,
self-hosted **project-management suite**, owned in Talaria's own Postgres/Redis,
not proxied from mission-control.

### Added
- **Boards & teams**, shareable kanban boards (personal or team-owned), a
  consolidated **Board settings** modal (General / People / Agents), board-scoped
  agent policy (restrictive by default), teams + member management, and soft
  **archive** for boards and tickets.
- **Tickets**, rich detail modal: TipTap WYSIWYG description (markdown under the
  hood) with read/edit toggle + slide-in full-screen editor, syntax-highlighted
  code + hover links, comments (Ctrl+Enter to send), an **Activity** tab, watchers,
  and a quality-review approval gate.
- **Ticket routes**, each ticket is a directly-linkable nested route
  (`/boards/:boardId/:taskId`) with copy-link buttons on cards, list rows, and the
  modal.
- **Fields**, agent-appropriate **effort** (XS-XL), **multiple assignees**
  (board-scoped agents only), ticket **dependencies** (blocked-by / blocks), labels,
  due date, and **auto-accumulated time-spent** (`addTimeSpentSeconds`). Manual hour
  estimates removed.
- **Blocked status**, a new kanban column for stalled / needs-input work.
- **List view**, configurable, drag-reorderable, click-to-sort columns, persisted
  per board.
- **Multiplayer**, live boards via Redis pub/sub → SSE (`/api/boards/:id/events`).
- **Reusable UI primitives**, `CloseButton`, `CopyLinkButton`, `InlineCreate`,
  `danger` button variant; `RichEditor` gains `bare` / `fill` / `onSubmit` modes.
- **Agent guardrails**, on `PUT /api/tasks/:id`, agents may triage but cannot
  self-assign (`assigned` → 403) or self-complete (`done` → `quality_review`), and
  cannot change assignees.
- **Agent MCP (`talaria-mcp`)** ([`mcp/`](./mcp)), an MCP server exposing only the
  safe tools (`list_boards`, `list_tickets`, `get_ticket`, `create_ticket`,
  `triage_ticket`, `comment`, `report_outcome`, `add_time`, `add_dependency`) — no
  assign, no complete; guardrails hold by construction. Identity via
  `TALARIA_AGENT_KEY` + `TALARIA_AGENT_NAME`.
- **Agent-authed task API**, the fleet key plus a new `x-agent-name` header opens
  an agent path on boards list, board tasks (list + create → `inbox`, never
  assigned), ticket detail, comments, and add-dependency. Every agent call is
  checked against the board's agent policy and attributed to the named agent in
  activity/comments. Named agents on `PUT /api/tasks/:id` are policy-checked too;
  unnamed key callers (legacy plugin heartbeat/report) keep their old access.
- **Group chat (channels)**, Slack-style channels where teammates and fleet
  agents are members. Channels + members + agents + messages live in Talaria's
  Postgres; live over Redis pub/sub → SSE (`/api/channels/:id/events`). Agents
  reply when **@mentioned** (by name or model id): the reply streams into the
  channel for every member, built from the channel transcript via the gateway
  plane. Composer has @mention autocomplete; channel settings manage people +
  agents (adding an agent requires access to it). New "Channels" nav surface.

- **People pickers**, one searchable `UserPicker` (over `GET /api/users`, everyone
  who has signed in) replaces every type-an-email field: board sharing, board
  creation invites, teams, and channel members.
- **Label picker**, ticket labels are tag chips + the shared combobox: the board's
  existing labels surface for reuse, and typing creates a new one (Enter or comma).
  Replaces the raw comma-separated text input.
- **Display names**, users can set how they appear (Settings → profile; updates the
  live session, no re-login). Member lists, channel messages, and avatars prefer
  the name and show the email as secondary.
- **Consistent control sizing**, one `sm`/`md` scale (`h-9`/`h-11`) shared by
  Button, Input, Select, and Combobox via a `size` prop — mixed-height form rows
  and hand-set `h-8`/`h-9` overrides are gone.
- **Notifications + user @mentions**, the channel composer autocompletes human
  members alongside agents; @mentioning a person drops a notification in their
  **Inbox** (new nav surface with an unread badge). `GET/PUT /api/notifications`;
  mention tokens are the email localpart, dashed name, or first name.
- **Token ledger**, every agent generation (1:1 chat turn + channel reply) lands
  in `usage_events` — real gateway-reported counts (`stream_options.include_usage`),
  or char-based estimates flagged `~` when the gateway doesn't report. The `/cost`
  page is live: today/7d/30d token tiles, a 14-day daily strip, and a per-agent
  breakdown. Dollar cost lands with per-LLM pricing attribution (see ROADMAP).
- **Admin console**, `/admin` is live: everyone who has signed in, with role
  management (member/admin; `AUTH_ADMIN_EMAILS` admins are pinned, no
  self-demotion) and the per-person agent allow-list UI (empty = all agents) that
  the access model always supported. Admin-gated `GET/PUT /api/admin/users`.
- **Agent harness (phase A)**, Talaria starts becoming the fleet's source of
  truth: `llm_endpoints` (model backends classed local/cloud — feeds the coming
  cost split), `agent_defs` + immutable `agent_versions` (soul, main model,
  `model_aliases` tiers, fallbacks, plugins, MCP servers — diffable, revertible),
  and an idempotent importer that ingests the existing `ai/orchestration` stack
  (`agents.yaml`, per-agent `config.yaml` + `SOUL.md`). `/agents` grows an
  admin-only **Definitions** panel showing each agent's tiers (local/cloud chips),
  fallback chain, soul, and version. Rendering + spin up/down land in phase B.
- **Agent harness (phase B)**, Talaria renders and runs the fleet: versions
  materialize into a gitignored `fleet/` dir (per-agent `config.yaml` emitted as
  YAML 1.1 so PyYAML sees exactly the original semantics, `SOUL.md`, a generated
  compose that reuses the legacy `ai_hermes-<dept>` volumes so memories survive,
  and the gateway manifest — which the bridge now **hot-reloads**, no restart).
  `/agents` gains live container status and lifecycle buttons: start/stop for
  managed agents, one-click **Migrate** for legacy ones (stop old → render →
  start managed → health-gate). Pilot migrated: `sam-support` runs Talaria-managed
  with memories intact, answering through the gateway.
- **Agent harness (phase C1: config control)**, edit an agent **in-app** — soul,
  main model, alias tiers, fallback chain, all against the endpoint registry —
  and save as a new immutable version; **Save & apply** re-renders and restarts
  the managed container. Reverts re-publish an old payload as a new version
  (history is append-only). Structured edits are merged into the raw Hermes
  config so rendering stays faithful.
- **Local vs cloud in the ledger**, every generation now records the serving
  model's endpoint class (from the agent's current main endpoint) + model id.
  `/cost` shows the 30-day local/cloud share bar, a stacked per-day strip, and a
  per-agent "% local" column — the view for optimizing the small-model/frontier
  mixture.
- **Models tab**, a System-area registry for model backends: one-click presets
  for **every common US provider** (Anthropic, OpenAI, Google, xAI, Meta,
  OpenRouter, Groq, Together, Fireworks, Cerebras, Perplexity, DeepInfra,
  DeepSeek, local Ollama/vLLM) with base URLs and wiring preconfigured — pick,
  name the key env var, done. Local/cloud is **inferred** (never asked for known
  providers; LAN/loopback URL heuristic for custom). Provider marks throughout;
  the provider chooser and the model tier picker are the same searchable
  combobox. Each provider card offers its **live catalog** (server-side
  `/models` fetch, keys never leave the box) so you search what the provider
  actually serves; per-provider **model catalogs** (tag-style add/remove) and
  cloud pricing fields.
  The agent editor's clunky selects are replaced by **one searchable picker**
  over every catalog. Deleting a model or provider that agents still use warns
  with the blast radius and **double opt-ins**, then cascades: each affected
  agent gets a new version with the tier stripped (revertible), re-rendered,
  running managed agents restarted. A model that is some agent's **main** is
  never cascaded — reassign first.
- **Agent harness (phase C2: create/retire)**, spin agents up and down on a
  whim. **New agent** on `/agents`: pick a template (any existing agent — model
  tiers/tools/plugins carry over with every identity reference re-stamped to the
  new slug), Talaria allocates a fresh gateway key into the stack `.env`, writes
  v1 with a starter soul, renders a fresh-chassis service (own state volume),
  starts it, and the bridge picks it up live. **Retire** removes the container
  and drops the agent from the fleet manifest; state volume + version history
  stay.

- **Pricing**, real dollars in the ledger: per-model $/MTok prices live on each
  provider (Models page grid; endpoint-level rates as fallback; Anthropic preset
  ships with official prices), every generation records its serving endpoint,
  and cost computes at read time — editing a price reprices history instantly.
  `/cost` gains a **Cloud spend** tile (30d + today), per-model $ in the split
  legend, a per-agent $ column, and a loud warning for **unpriced** cloud tokens
  (never silently $0). Local tokens are $0 by definition.
- **Model-tier routing**, chat any agent on any of its configured tiers: the
  fleet manifest now carries one gateway entry per alias (`<base>-<alias>`,
  resolved by the agent's own Hermes gateway), `/api/agents` returns real
  agents (not raw gateway models) with their tiers, the chat composer gains a
  tier select, and the API validates tiers against the agent's definition. The
  ledger attributes tier-routed turns by the **alias's** endpoint — a `glm`
  turn lands as cloud/glm while main-model turns stay local.
- **Auto-fetched pricing**, zero-config rates: a server-side price oracle pulls
  OpenRouter's public model catalog (no key) and prices every matched cloud
  model automatically (`llm_endpoints.auto_prices`, refreshed in the background
  and on provider/model changes). Cost coalesces user override → auto → endpoint
  default; the pricing grid shows an "auto" tag with fetched rates as
  placeholders. No exact match → honestly unpriced, never guessed.
- **Channel tier mentions**, `@Dex:deepseek` routes that reply to the tier; the
  composer autocompletes `Label:tier`; unknown tiers fall back to main; the
  ledger attributes the turn to the alias endpoint.
- **Activity** (`/activity`), one merged, user-scoped feed — ticket events,
  channel messages, agent config versions — with kind filters. A read model
  over existing tables; nothing new stored.
- **Alerts** (`/alerts`), live-derived health: down/unhealthy managed
  containers, unreachable gateway plane, unpriced cloud usage,
  estimate-dominated ledger, failed and week-stale blocked tickets. Severity
  ranked, deep-linked, nothing to configure.
- **Skills** (`/skills`), the fleet's skills as they exist on disk (shared
  stack dir + each agent's dept/fleet mount): parsed descriptions, live
  SKILL.md editing, admin create/delete. Hermes reads skills per invocation,
  so edits apply on the next run — no restart.
- **Memory** (`/memory`), each managed agent's `memories/MEMORY.md` read and
  written through its running container — no second copy to drift.
- **MCP** (`/mcp`), per-agent MCP servers from the versioned config: add and
  remove as NEW immutable config versions (optionally applied live), untouched
  entries preserved byte-for-byte; plus a talaria-mcp explainer.
- **Inference** (`/inference`), your own hardware: local backends probed live
  (status, latency, serving-now models) plus local token throughput.
- **Per-ticket token spend**, agents report tokens burned on a ticket
  (`POST /api/tasks/:id/usage`, MCP tool `log_usage`, board policy enforced,
  tier-aware); reports are first-class priced ledger rows; the ticket rail
  shows tokens · $ · per-model.
- **Plan chat**, a channel's **Plan** button turns the conversation into
  tickets: a chosen agent (any tier) drafts structured proposals from the
  transcript, a human edits/prunes them in a review modal and creates the
  keepers — into inbox, never assigned.

### Fixed
- SSE event streams no longer crash the server when a client disconnects before
  the Redis subscriber finishes connecting (unhandled rejection in the
  board/channel event stream).
- Ticket labels no longer require hand-typed comma lists (see label picker).
- Logging in no longer clobbers a user-set display name (the provider identity
  only fills the unfriendly defaults).

### Changed
- Sessions are Redis-backed (opaque sid → `sess:<sid>`), not HMAC cookies.

## [Unreleased]: 0.1.0

Initial working slice: hermes-workspace ↔ mission-control bridge + per-agent adapter plugin.
All milestones below verified live against a running stack on 2026-07-01
([`scripts/verify-stack.sh`](./scripts/verify-stack.sh), all checks pass).

### Added
- **Gateway plane** (`bridge/src/gatewayPlane.ts`), the fleet multiplexer. Fronts the Hermes gateway
  `:8642` for a whole fleet: `/v1/models` = every agent, `/v1/chat/completions` routed by model to that
  agent's real gateway (per-agent key, SSE streamed). One workspace talks to every agent via the model
  switcher. Fleet declared in a manifest (`TALARIA_FLEET`/`TALARIA_FLEET_FILE`). Stood up the Phase-1
  fleet engine as a two-plane runtime (gateway multiplexer + dashboard management bridge).
- **Bridge** (`bridge/`, Node/TS), transparent reverse-proxy of the Hermes dashboard `:9119`.
  - **M1** pass-through: all 164 dashboard routes (incl. OAuth + 4 websockets) proxied byte-for-byte
    to the real dashboard; conductor capability-probe (`GET /api/conductor/missions`) served as
    `200 application/json` so the workspace uses remote dispatch instead of native-swarm.
  - **M2** mission create: `POST /api/conductor/missions {name,prompt}` → mission-control
    `POST /api/tasks`, response shaped for the Conductor.
  - **M3** status round-trip: `GET/DELETE /api/conductor/missions/{id}` poll + cancel, mapping
    mission-control task status → the workspace mission enum. Never forces the Aegis-gated `done`.
- **Fleet board**, the bridge serves the workspace's `/api/plugins/kanban/*` surface from
  mission-control, so the swarm/kanban board becomes a live view of the MC fleet (columns = MC
  statuses, cards = MC tasks, full CRUD; `done` stays Aegis-gated). Toggle `TALARIA_KANBAN_FROM_MC=0`.
  The conductor poll's `lines` are also enriched with the task's header + comment feed.
- **Plugin** (`plugin/talaria/`, Hermes standalone), per-agent mission-control adapter.
  - **M3** register (`POST /api/agents/register`) + opt-in background heartbeat
    (`TALARIA_HEARTBEAT_SECONDS`) that polls `/api/agents/{id}/heartbeat` + reports via
    `PUT /api/tasks/{id}` (toward `quality_review`, never `done`). Safe no-op until configured.
- **mission-control adapter** (`adapter/`), **M4** `HermesAdapter` making Hermes a first-class
  framework in mission-control; PR-ready patch + verification.
- **Stack** (`stack/`), compose wiring workspace + mission-control + bridge on the shared `edge`
  network; `scripts/verify-stack.sh` reproduces the M1-M3 verification end-to-end.
- **Docs**, [`docs/m0-contract.md`](./docs/m0-contract.md) (the M0 contract diff + `:9119` allowlist),
  README with architecture + compatibility matrix.
- **Fleet: both Hermes deployment shapes** (`bridge/src/config.ts`, `gatewayPlane.ts`, `sessions.ts`) -
  a fleet entry now supports (A) separate installs (one gateway per agent) and (B) multiple Hermes
  profiles on one host (each profile's API server on its own port). Optional `profile` / `upstreamModel`
  / `pathPrefix` fields: Talaria rewrites the forwarded `model` to the profile when set and honours a
  profile path prefix on chat + session calls. The UI only ever sees Talaria's exposed model ids.
  `pathPrefix` seeds support for Hermes' emerging single-endpoint profile multiplex (`multiplex_profiles`).
- **Phase 2 UI** (`ui/`), the first slice of Talaria's own front end (Vite + TanStack Start, React 19,
  TypeScript), matching the hermes-workspace stack so its chat components lift cleanly.
  - **Mercury design system** (`ui/src/styles.css`, `ui/src/lib/theme.ts`), hand-rolled Tailwind v4
    tokens, dark (`mercury`) + light (`mercury-light`), violet→magenta neon on Mercury-planet neutrals.
    Reuses hermes-workspace's `--theme-*` token contract to keep component lifts frictionless.
  - **Pluggable auth**, each provider independently enable-able (flag **+** secrets required):
    registry (`ui/src/server/auth/config.ts`), stateless HMAC-signed sessions, **Google OAuth**
    (start + callback) and username/password, with routes `/api/auth/{providers,session,google,
    google/callback,password,logout}`. Login screen renders only the enabled providers. Verified live:
    session round-trip, logout, tampered-cookie rejection, provider gating.
  - Both upstreams vendored under `vendor/` (gitignored) as lift sources.

### Key findings (verified against source)
- The Hermes dashboard has **no** `/api/conductor/*` routes, Talaria *serves* them (adds capability),
  it does not override native behavior. Unset `HERMES_DASHBOARD_URL` → 100% native.
- mission-control gates `done` behind **Aegis** approval; Talaria respects it (human-only Done).
- mission-control has no published image (builds from source, pinned `d09e608`).

### Not yet
- Decomposed/broadcast mission parity (those go workspace-local `:3000`), needs the
  `HERMES_MISSION_API_URL` upstream PR to hermes-workspace.
- Executing pulled work inside the Hermes run loop (heartbeat pulls; in-agent dispatch is next).
- Enabling the plugin on the live PackLedger fleet (staged, not yet `--force-recreate`d).

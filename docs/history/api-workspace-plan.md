# API crate split, debloat, and build times

Living plan for `api/` only. `desktop/src-tauri` is out of scope.

**Hardware note.** Phase 0 was supposed to land on the 8-core / 15 GB box. This
machine is already **32 cores / 62 GB**. Every number below is post-bump.
Compare later work with **relative deltas**, not vs an 8-core baseline.


Toolchain: `api/rust-toolchain.toml` → rustc **1.97.1**. Commands must run with
`cwd=api/` so rustup honors the pin. Host default rustc 1.88 is too old.

## Phase 0 — measured 2026-09-18

| Metric | Wall | Notes |
|---|---|---|
| Warm `cargo build` (existing `api/target`) | 2m 05s | `--timings` HTML in `api/target/cargo-timings/` |
| Cold `cargo build` (`CARGO_TARGET_DIR=/tmp/talaria-api-cold-target`) | **3m 10s** (190s) | Full dep graph + `talaria-api`; 32-way rustc |
| No-op `cargo build` | 0.38s | |
| Incremental after `touch src/routes/mod.rs` | **23.7s** | whole crate codegen |
| Incremental after `touch src/error.rs` | **20.4s** | |
| Warm `cargo check` | 62s | |
| Warm `cargo clippy --all-targets -D warnings` | 79s | green |

No `[profile.dev]` / linker override in `api/Cargo.toml` at Phase 0.

Success criteria later: same four rows + CI `api` job wall + package image wall.

## Phase 1 — landed 2026-09-18

`[profile.dev]` opt-level 1 / deps 3 / `debug = "line-tables-only"`. gnu linux
links with mold (`api/.cargo/config.toml`). CI and the devbox image install
`mold`. Package/musl stays on the default linker.

After numbers (32-core box, relative to Phase 0):

| Metric | Phase 0 | Phase 1 |
|---|---|---|
| First `cargo build` after profile invalidate | — | 4m 34s (deps at opt-level 3) |
| Incremental `touch src/routes/mod.rs` | 23.7s | **20.7s** |
| Incremental `touch src/error.rs` | 20.4s | **18.9s** |
| Warm clippy `--all-targets` (after test-target rebuild) | 79s | 0.5s (artifacts hot) |
| First clippy after profile | 79s | 2m 17s |

`ld.mold` must be on PATH (`mold` package). A `mold` binary alone is not enough — gcc collect2 looks up `ld.mold`.

P0.5 still blocked (tower live; dead_code not unused). Next is workspace scaffold after prune, or udeps.

## Phase 2a — started 2026-09-18

37 leaf crates. `talaria-fleet-layout` broke gateway↔fleet `fleet_dir`.
Gateway still touches `price_oracle`; fleet engine still in the monolith.










## Churn × LOC (git log `--numstat` since 2026-05-18, `api/`)

1112 files. Rank is `(added+deleted) × current LOC`. High rank = large *and*
heavily rewritten — not automatically dead.

| churn×loc | churn | loc | path |
|---|---|---|---|
| 52.4M | 7648 | 6848 | `src/fitness/surface.rs` |
| 43.4M | 6868 | 6326 | `src/fitness/evals.rs` |
| 35.8M | 6467 | 5533 | `src/fitness/probes.rs` |
| 28.6M | 5616 | 5090 | `src/harness/defs/muse.rs` |
| 24.0M | 5361 | 4483 | `src/harness/run.rs` |
| 18.7M | 4343 | 4295 | `Cargo.lock` |
| 17.6M | 4397 | 4003 | `src/fitness/adversarial.rs` |
| 17.6M | 4379 | 4015 | `src/harness/defs/research.rs` |
| 12.2M | 3604 | 3384 | `src/runs/defs/research.rs` |
| 10.7M | 3785 | 2817 | `src/tasks.rs` |
| 8.9M | 3159 | 2803 | `src/fitness/toolbox/sandbox.rs` |
| 8.7M | 3088 | 2808 | `src/fitness/score.rs` |
| 8.1M | 2976 | 2724 | `src/harness/defs/inbox_focus.rs` |
| 6.4M | 2732 | 2350 | `src/runs/defs/work_session.rs` |
| 6.4M | 2723 | 2341 | `src/notify.rs` |
| 6.1M | 2550 | 2378 | `src/daily_brief/mod.rs` |
| 4.4M | 2913 | 1505 | `src/routes/mod.rs` |
| 4.2M | 2290 | 1830 | `src/gateway/guard.rs` |
| 4.2M | 2096 | 1984 | `src/approvals.rs` |
| 3.3M | 1878 | 1764 | `src/body.rs` |

Fitness + harness defs dominate churn. Root singles (`tasks`, `notify`,
`approvals`, `body`) are still hot, not abandoned. Bucket B still needs a
**product** call; this ranking does not justify deleting those engines.

## Corrections to the recon

- **`tower` is not dead.** `tower::ServiceExt` is used in live/integration tests
  (`api/tests/llm_models.rs`, `ticket_threads_live.rs`, `uploads_live.rs`,
  `workchains_live.rs`). Do not remove.
- **`#[allow(dead_code)]` is not 12 unused items.** Sites are serde-selected
  fields, fixture `name`s, and one test helper (`integrations_google_drive_manage.rs`
  `_unused`). Strip only with a zero-caller proof, per deletion guardrail.
- **Incremental pain is the monolith:** a one-line touch of `routes/mod.rs`
  still pays ~24s of `talaria-api` codegen. Split helps full/CI builds; this
  number is the dev-loop bar for Phase 5.

## Open decisions (recommendations, used unless overruled)

1. ~14 crates, coarse at the routes boundary — not Feldera-style 1000.
2. CI check job: Swatinem/rust-cache only; no sccache stack.
3. mold in the package image only after musl-mold check.
4. Bucket B owner: product, off the critical path.

## Sequencing (unchanged)

Prune before split. Consolidate after. One crate per PR, `bun run api:check`
green each step, move-only during Phase 2.

P0 this file. Next: P0.5 mechanical prunes (not `tower`); P1 `profile.dev` +
linker; then workspace scaffold.

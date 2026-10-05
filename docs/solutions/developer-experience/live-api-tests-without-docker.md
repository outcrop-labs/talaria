---
title: Running the live API tests and migration replay on a box without Docker
date: 2026-10-05
category: developer-experience
module: api/tests/it
problem_type: developer_experience
component: development_workflow
severity: medium
applies_when:
  - "A change touches api/** or ui/src/server/db/** and the #[ignore]d live tests or the migration replay must run before the PR"
  - "docker.sock is permission-denied (a devbox user outside the docker group) so bun talaria worktree cannot start its Postgres and Redis"
  - "No system Postgres, Redis or pip is installed"
tags: [live-tests, postgres, redis, embedded-postgres, migrations, devbox, docker]
---

# Running the live API tests and migration replay on a box without Docker

## Context

The live API suite (`api/tests/it/`, every module marked `#[ignore]`) and the migration replay (`bun run migrations:check` in `ui/`) both need a real Postgres and Redis. CI provides them as `postgres:16-alpine` and `redis:7-alpine` containers in `.github/workflows/api-integration.yml`. Locally, `bun talaria worktree` starts the same pair through Docker.

On a devbox where `docker.sock` is permission-denied and nothing is installed system-wide, neither path works. The tests then ship unexecuted, and CI becomes the first place they run. Both Postgres and Redis can instead run as plain user processes in `/tmp`, with no root and no Docker.

## Guidance

**Postgres** comes from the npm package `@embedded-postgres/linux-x64`, installed into a scratch directory. Bun blocks the package's postinstall, so run its symlink step by hand:

```bash
mkdir -p /tmp/pgbin && cd /tmp/pgbin && echo '{}' > package.json
bun add @embedded-postgres/linux-x64          # "Blocked 1 postinstall" is expected
cd node_modules/@embedded-postgres/linux-x64 && node scripts/hydrate-symlinks.js
P=/tmp/pgbin/node_modules/@embedded-postgres/linux-x64/native/bin
echo talaria > /tmp/pwfile
$P/initdb -D /tmp/pgdata -U talaria --pwfile=/tmp/pwfile -A md5
$P/pg_ctl -D /tmp/pgdata -o "-p 55432 -k /tmp" -l /tmp/pg.log start
# then create the database `talaria` with any client, e.g. the `postgres` package from ui/node_modules
```

**Redis** builds from source in about a minute (gcc and make are present on the devbox):

```bash
cd /tmp && curl -sfL -o redis.tgz https://download.redis.io/releases/redis-7.2.5.tar.gz
tar xzf redis.tgz && cd redis-7.2.5 && make -j8 BUILD_TLS=no
src/redis-server --port 56379 --daemonize yes --save "" --appendonly no
```

**Environment.** Keep `TALARIA_SECRET_KEY` the same across every run against a given database:

```bash
export DATABASE_URL=postgres://talaria:talaria@127.0.0.1:55432/talaria
export REDIS_URL=redis://127.0.0.1:56379
export TALARIA_SECRET_KEY=$(head -c32 /dev/zero | base64)   # any value, but the SAME one every run
```

The first migration run seals a data key under that root key. Run with a fresh random key and the next run fails in `talaria-secretbox` (`api/crates/talaria-secretbox/src/lib.rs:223`) with "this database has 1 data key(s) and none can be unwrapped with the current root secret".

**Migration replay.** Run it from `ui/`:

```bash
PG_DUMP=false bun run migrations:check
```

The replay itself (step 1 of `ui/scripts/check-migrations.ts`) completes, and the script then fails at the `pg_dump` step. That failure is expected for two reasons:
- The default dump command is `docker exec talaria-mig-pg pg_dump` (`ui/scripts/check-migrations.ts:54`).
- The embedded package ships no `pg_dump`, and it is PostgreSQL 18, while the snapshot is cut from 16.

Reaching the dump step proves every migration applies. The `schema.snapshot.sql` diff stays CI's check, so a hand-edited snapshot is verified there.

**Live tests.** Run them from `api/`, scoped to the modules you touched:

```bash
cargo test -q -p talaria-api --test it -- --ignored profile_live:: conversation_reactions_live::
```

## Why This Matters

Without this, live-database tests written on a no-Docker devbox are committed unexecuted, so the first real run is CI's `api-integration` job after the push. Running them locally caught nothing new this time (17/17 passed), but it turned "written, not executed" into evidence before the PR opened.

## When to Apply

- Before pushing a change to the Rust API's SQL, routes or the `MIGRATIONS` array from a box where `docker ps` is denied.
- When a full `--ignored` run shows failures in modules you did not touch.
  - Rerun those modules on a **fresh** database before reading them as regressions. Leftover rows from earlier runs, plus a rotated `TALARIA_SECRET_KEY`, made `gateway_hot_caches`, `typed_binds` and `workchains_live` fail on a reused database; all three passed on a new one.
  - `llm_models::minted_key_lists_the_catalog` always needs `TALARIA_LLM_TEST_KEY` and is excluded from CI for that reason.

## Examples

The run that produced this note (branch `t3code/45083f68`, comms Slack-style polish):
- The 17 new live tests for profile photos, presence, agent-DM reactions and threads/sent passed against the embedded Postgres and the source-built Redis.
- The full suite on a reused database showed 4 failures, all unrelated to the change. On a fresh database the 3 rerun modules passed (21 tests). The fourth failure is the LLM test, which needs the key above.

## Related

- `docs/WORKTREES.md` and `docs/DEVBOX.md`: the Docker-backed paths this replaces when Docker is unavailable, and why the root key must stay constant.
- `.github/workflows/api-integration.yml`: what CI runs, and which live modules stay local-only.

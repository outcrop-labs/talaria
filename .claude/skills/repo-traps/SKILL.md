---
name: repo-traps
description: The repo's known operational traps, each "if you see X, check Y" — Rust bind-cast 500s, vite serving a stale negative after a bad import, a killed api still ticking its scheduler, docker daemon DNS, and minting a Redis session for authed curl. Use when a change that should work fails oddly, a port or process misbehaves, or an API-level test needs authentication.
---

# Repo traps

Each trap below cost real time once. They are phrased symptom → check → fix, and each is
environment-neutral — the shape generalizes even when the incident that earned it was one
machine's.

## 1. A query 500s that looks right

**Symptom:** a Rust query bind compiles, reads fine, and 500s at runtime.

**Check:** the casts against the column's actual type. Postgres does not coerce what sqlx
leaves untyped:

- `$N::uuid` cast against a column that is **TEXT** — the cast demands a uuid-shaped
  string; a bare id column of text 500s under it.
- A missing `$N::timestamptz` on an epoch-milliseconds bind feeding an epoch-to-ISO SQL
  helper — the bind lands as something else and the comparison explodes.

**Fix + prevention:** correct the cast, then sweep the module you touched — every bind,
against `information_schema.columns`, per cutover-module practice (the recorded rule lives
in docs/RUST-MIGRATION.md). One wrong cast in a module predicts a second.

## 2. The whole API 500s after a route/import edit — and keeps 500ing after the fix

**Symptom:** every `/api/*` request 500s following a route or import change; you fix the
bad import; it *still* 500s.

**Check:** nothing — restart first. Vite's dependency optimizer can serve a stale negative
off a single bad route import, and the stale negative outlives the fix that caused it.

**Fix:** restart the dev server. If it persists, clear `ui/.vite` (the optimizer cache)
and restart again. Debugging the code before restarting is the trap.

## 3. Port 5274 looks free, but runs fail with bugs your new binary doesn't have

**Symptom:** nothing bound on :5274, yet background runs execute with old-binary behavior
or fail with errors your current code can't produce.

**Check:** for a live api process that never died — `pgrep -af talaria` / `ps aux | grep
target`. A "killed" api can keep ticking its scheduler with no port bound: the port freed,
the process didn't, and its scheduler keeps claiming work and running it on the old code.

**Fix:** kill it for real (`kill -9` if it shrugs), then start the fresh process. Know the
arming switch while you're in here: boot arms the scheduler unless `TALARIA_SCHEDULER` is
exactly `off` — that is the only value that disarms (api/src/scheduler.rs). Set `off` to
run routes without background jobs; unset it for the process that should own the schedule.

## 4. Docker builds fail with "Temporary failure resolving…"

**Symptom:** image builds die on hostname resolution while the host resolves fine.

**Check:** the docker daemon's resolver config (`daemon.json`) — a pinned upstream DNS
that nothing answers leaves every bridge-network build dead.

**Fix:** builds can pass `--network=host` to bypass the dead resolver path
(`bun talaria box build` already does). Runtime containers that need to reach names may
need explicit `dns:` pins on their network. More: docs/DEVBOX.md, docs/CONTAINER.md.

## 5. Authed curl without a browser

**Symptom:** you need to exercise a session-authenticated API path and there is no browser
in the loop.

**Recipe:** mint the session directly in Redis — the cookie carries only an opaque id, the
user record lives under the key:

```bash
redis-cli -u "$REDIS_URL" SET "sess:<sid>" '<json>' EX 604800
curl -H 'Cookie: talaria_session=<sid>' http://localhost:5273/api/<route>
```

The JSON field **order is part of the contract** (the struct is declared in stored order;
see api/src/session.rs): `id, sub, email, name, picture, role, provider` — `role` is
`admin` or `member`, `provider` is `google` or `password`, and `email`/`name`/`picture`
are nullable. `<sid>` is any unguessable string you choose; TTL 7 days matches the real
thing. `REDIS_URL` comes from `ui/.env` — in a worktree, that stack's own Redis on `65xx`.
api/src/session.rs (`SESSION_COOKIE`, `key()`) is the source of truth; re-verify against
it before relying on the field list.

---

**Maintenance rule:** this list is earned experience, not theory. When you hit a NEW trap
with a generalizable check, add it here — symptom, check, fix — and keep the incident
details out; the shape is the skill.

# Per-agent credentials — the migration and its one-way door

Every agent authenticates with **its own** `tak_` secret (minted per `agent_defs` row, sealed in
`agent_keys`, stamped into that agent's container as `TALARIA_AGENT_KEY` by fleet-render). The
org-wide `TALARIA_AGENT_KEY` names nobody, so it is being retired. `agent-auth.ts` owns both sides.

## What a legacy caller is, and is not

A caller presenting the org-wide key resolves `legacy: true`. It is **identified but untrusted**:

| | shared key (`legacy: true`) | own credential |
|---|---|---|
| board/channel work its policy allows | yes | yes |
| named identity | resolved against `agent_defs` — an unknown or **retired** name is a 403 | proven by the credential |
| org-wide elevation (all boards, all non-DM channels, non-private KB/artifacts) | **no** | if elevated |
| acting as its owner (`actingUser`, owner's boards, owner's Google token) | **refused at the door** | yes |

A name that carries human privilege — a personal assistant (`owner_user_id`) or an `elevated`
assistant — **cannot be asserted at all**. That agent gets a 403 naming its container and the
variable to stamp, because every capability such an identity has is escalation; there is no
"ordinary scoped work" left to keep serving. General agents are unaffected: this is why the window
stays open by default.

## Migration order — do not improvise it

```
1. migrate            deploy + boot the new build; MIGRATIONS creates agent_keys
2. render the fleet   mints + stamps TALARIA_AGENT_KEY_<SLUG> into fleet/.env
3. roll the running   each container is RECREATED and picks up its own credential
   containers
4. every agent seen   agent_keys.last_used_at is set for all of them
5. TALARIA_AGENT_KEY_LEGACY=off
```

**Flipping the flag before step 4 is a fleet-wide outage** — every un-rolled container 401s at once.
Nothing can stop an operator editing an env var, so the order is guarded by making step 4
observable:

- **`legacyMigrationStatus()`** (agent-auth) reports, per enabled managed agent, whether its key is
  minted and when it last authenticated with it. `pending` is the list that is not migrated yet.
- Every **fleet render** pushes that as a warning (shown with the render result) and logs it:
  `per-agent credential migration: 3/5 done. Still on the org-wide key: …`
- With the flag already off and agents pending, the same line is logged at **error** level and says
  they are locked out right now.
- A shared-key caller is logged (per agent, every 15 min while it keeps happening) and recorded in
  `legacyUsage()` for this process.

---

# The deploy runbook

Do these in order. Steps 1→3 are one maintenance window; step 5 is a separate, later decision.

## Step 0 — Preflight

**Who breaks at deploy?** Personal and elevated assistants are refused the moment the new code is
live — not at the flag flip (the `def.personal || def.elevated` branch in `resolve()`,
`agent-auth.ts`). Everyone else keeps working as a legacy caller. Get the list first:

```sh
psql "$DATABASE_URL" -c "
  select model, owner_user_id is not null as personal, elevated
    from agent_defs where enabled and managed order by model"
```

Any row with `personal` or `elevated` true is an agent that will 403 with
`{"error":"this agent must present its own credential"}` on every toolkit call from the deploy until
you roll its container. On the reference install that is exactly one agent, `dan-personal-dan`; the
other four resolve legacy and keep working.

**The window must be open.** Absent means on (`legacyOpen()` in `agent-auth.ts` reads
`process.env.TALARIA_AGENT_KEY_LEGACY ?? 'on'`):

```sh
grep -n '^TALARIA_AGENT_KEY_LEGACY=' ui/.env || echo 'absent → on (the default). Good.'
```

If that prints `=off`, **stop** — set it back to `on` (or delete the line) before deploying, or every
agent 401s the moment the build lands.

**Take a snapshot.** `./scripts/backup.sh` ([BACKUPS.md](./BACKUPS.md)). The migration is additive,
but the roll recreates containers.

## Step 1 — Migrate

Deploy the build and restart the app. Migrations run on the app's **first database query**
(`db()` → `ensureMigrated()` → `runMigrations()`, `db/pg.ts`), under an advisory lock, checksummed
and append-only — there is no separate migrate command to run.

`GET /api/healthz` deliberately uses `getSql()` rather than `db()` and **does not** trigger
migrations (`routes/api/healthz.ts`), so it proves the process is alive and nothing more. Any real request
(signing in, step 2's login) runs them.

Check:

```sh
curl -fsS http://127.0.0.1:5273/api/healthz            # status: ok
psql "$DATABASE_URL" -c "select count(*) from agent_keys"   # table EXISTS, 0 rows
```

`relation "agent_keys" does not exist` means the app has not served a query yet — hit the app once
and re-check. Do not continue past this line without the table: `renderFleet` inserts into it.

**From this moment personal and elevated assistants are down** — see
[the section below](#personal-assistants-stop-proxying-the-moment-this-deploys). Minimise this window
by going straight to steps 2 and 3.

## Step 2 — Render the fleet

The render mints one `tak_` credential per managed agent, writes
`TALARIA_AGENT_KEY_<SLUG>` into `fleet/.env`, and rewrites `fleet/docker-compose.yml` so each service
interpolates its own variable (`ensureAgentEnvKeys` and the `env.TALARIA_AGENT_KEY =
${AGENT_KEY_VAR(def.slug)}` line in the service render, `fleet-render.ts`).

```sh
# admin session
curl -sS -c /tmp/talaria.cookies -X POST http://127.0.0.1:5273/api/auth/password \
  -H 'content-type: application/json' \
  -d '{"username":"admin@example.com","password":"…"}'

curl -sS -b /tmp/talaria.cookies -X POST http://127.0.0.1:5273/api/fleet/render
```

(Any agent lifecycle action that renders — Start, Re-hire — does the same thing; there is no
dedicated Render button in the UI today.)

Check all three:

```sh
grep -c '^TALARIA_AGENT_KEY_[A-Z0-9_]*=' fleet/.env        # one per enabled managed agent
grep -n 'TALARIA_AGENT_KEY' fleet/docker-compose.yml       # ${TALARIA_AGENT_KEY_<SLUG>} per service
psql "$DATABASE_URL" -c "select count(*) from agent_keys"  # same count as the .env lines
```

The render response's `result.warnings` carries the migration line
(`per-agent credential migration: 0/5 done…`). At this point it should say **0 done** — nothing has
been rolled yet. That is correct, not a failure.

**Nothing has changed for the running containers yet.** Their environment was baked at container
create; they still hold the org-wide key. The render only changed files on disk.

## Step 3 — Roll the running containers

**Roll, not restart.** `restart` is `docker compose restart` (`fleetRestart` in `fleet-docker.ts`),
which does not recreate the container and therefore does **not** pick up the new environment — the
agent stays on the org-wide key while looking healthy. `POST /api/fleet/reconcile` is not enough
either: it skips anything already running (`reconcileFleet` in `fleet-reconcile.ts`). Only `roll`
replaces the container (`rollAgent`, same file).

**Personal and elevated assistants first** — they are the ones that are currently down:

```sh
psql "$DATABASE_URL" -At -F' ' -c "
  select id, model from agent_defs
   where enabled and managed and (owner_user_id is not null or elevated) order by model"
```

Then everything else:

```sh
psql "$DATABASE_URL" -At -F' ' -c "
  select id, model from agent_defs
   where enabled and managed and owner_user_id is null and not elevated order by model"
```

Roll each id, one at a time, waiting for each to come back before starting the next:

```sh
curl -sS -b /tmp/talaria.cookies -X POST \
  http://127.0.0.1:5273/api/fleet/agents/<AGENT_ID>/control \
  -H 'content-type: application/json' -d '{"action":"roll"}'
```

The call returns `{"ok":true,"rolling":true}` immediately and the work continues detached: a fresh
container comes up on a new port, health is waited for, the manifest cuts over, and the old container
drains for `TALARIA_ROLL_DRAIN_SECONDS` (default 45) before it retires. A replacement that never
becomes healthy is discarded and the old container keeps serving — which also means **a failed roll
leaves that agent on the org-wide key**. The equivalent UI path is Agents → the ⟳/Repeat icon
("Roll — zero-downtime replacement"), admin only.

Check each rolled agent two ways. The container really holds its own credential:

```sh
docker inspect talaria-fleet-agent-<department>-1 \
  --format '{{range .Config.Env}}{{println .}}{{end}}' \
  | grep -q '^TALARIA_AGENT_KEY=tak_' && echo "own credential" || echo "STILL ON THE ORG KEY"
```

(A rolled agent lives in the `-b-1` container until its next roll — `docker ps` tells you which.)

And Talaria has actually *seen* it authenticate with that credential:

```sh
psql "$DATABASE_URL" -c "
  select d.model, k.last_used_at
    from agent_defs d join agent_keys k on k.agent_id = d.id
   where d.enabled and d.managed order by d.model"
```

## Step 4 — Wait for every agent to be seen

`last_used_at` is written on every per-agent authentication (the fire-and-forget
`update agent_keys set last_used_at = now()` in `resolve()`, `agent-auth.ts`). A `null` means that
agent has minted a key but has never presented it — the flag would lock it out. Re-render and read the
warning if you'd rather have it in one line:

```sh
curl -sS -b /tmp/talaria.cookies -X POST http://127.0.0.1:5273/api/fleet/render
# → result.warnings: "per-agent credential migration: 5/5 done" is ABSENT when done.
#   Any remaining line names exactly who is still pending.
```

Give a rolled agent a heartbeat interval (default 45s, `TALARIA_HEARTBEAT_SECONDS`) to show up.

## Step 5 — Close the window

Only with zero pending agents:

```sh
echo 'TALARIA_AGENT_KEY_LEGACY=off' >> ui/.env
# restart the app
```

Check: the render warning stays silent, and the org-wide key now 401s with
`the org-wide agent key is retired`. If anything was still pending, the next render logs at **error**
level that those agents are locked out right now — set the flag back to `on`, restart, and finish
step 3.

## Rolling back

| Where you are | How to get back |
|---|---|
| Flag flipped, agents locked out | `TALARIA_AGENT_KEY_LEGACY=on` (or delete the line) + restart the app. Un-rolled containers authenticate again as untrusted callers. **This does not restore personal/elevated assistants** — they are refused regardless of the flag; only rolling their container (or reverting the build) brings them back. |
| Rolled containers misbehaving, build staying | Roll again, or `stop` + `up` the agent — both recreate from the current render. |
| Reverting to the previous build | Revert, restart the app, then **re-render and re-roll every agent you already rolled**. The previous build's `agent-auth.ts` only knows the org-wide key, so a container still presenting `tak_…` 401s until its compose service is re-rendered back to `${TALARIA_AGENT_KEY}`. |
| The `agent_keys` table after a revert | Leave it. `runMigrations` iterates the array and ignores `schema_migrations` rows beyond its end, so the extra statement is inert under the old build and is not replayed when you roll forward again. |

---

## Personal assistants stop proxying the moment this deploys

**Say it plainly: between the deploy (step 1) and the roll (step 3), a personal or elevated assistant
cannot act for its owner, and cannot use the toolkit at all.** This is not limited to `actingUser`
— `resolve()` refuses the caller at the door, before any surface gets to decide (the
`def.personal || def.elevated` branch, `agent-auth.ts`), so every toolkit call from that container
403s with

```json
{"error":"this agent must present its own credential"}
```

The failure is **diagnostic, not silent**. The body carries the fix, and the same line is logged at
error level (every 15 minutes while it keeps happening):

> "dan-personal-dan" acts for a human (personal assistant / elevated), so the org-wide
> TALARIA_AGENT_KEY cannot authenticate it — it proves fleet membership, not identity. Re-render the
> fleet and roll this container so it presents TALARIA_AGENT_KEY_DAN.

If you run personal assistants, roll them **first** — step 3 orders them that way for exactly this
reason. Nothing else shortens the outage: the legacy flag does not help (the refusal is unconditional
while the window is open *and* after it closes), and a `restart` does not recreate the container.

## fleet/.env

`fleet/.env` holds every agent's plaintext credential. The renderer writes it `0600` in a `0700`
directory and re-locks both — `writeFleetEnv` chmods the file and its parent on every write, and
`ensureFleetEnvKey`/`ensureAgentEnvKeys` re-chmod even when there is nothing to append (all three in
`fleet-render.ts`). Anything looser lets any local account, or any workbench agent with a shell,
impersonate the whole fleet.

**That is render-time only, and it does not reach an install that hasn't rendered since.** A deployed
instance keeps whatever modes it already had (`0644` in a `0755` directory, the process umask
defaults) until a render happens. The same is true of per-agent `secrets.env`:
`materializeAgentSecrets` (`agent-secrets.ts`) chmods the file at write time and never touches its
directory (created `0755` by the per-agent `mkdir` in `fleet-render.ts`). So **existing installs need
a one-time chmod**, done by hand once:

```sh
cd /path/to/talaria
chmod 700 fleet
chmod 600 fleet/.env
chmod 600 fleet/agents/*/secrets.env 2>/dev/null   # only if any agent has secrets
```

Verify with `stat -c '%a %n' fleet fleet/.env` — you want `700` and `600`.

> **Follow-up (not done here):** `scripts/setup.sh`, or the app at boot, should apply that chmod so a
> new install and an upgraded one converge without an operator remembering. Today the only thing that
> locks these paths down is a render.

The DB is the source of truth: a `TALARIA_AGENT_KEY_<SLUG>` line is **rewritten** from `agent_keys`
on every render, never skipped because it exists. Skipping bricked two cases — a slug recreated
after `deleteAgentForever` (stale line, no `agent_keys` row → the container presents a dead secret
and 401s with no diagnostic), and a DB restore against a preserved `.env`. Deleting an agent now
strips both `HERMES_KEY_<SLUG>` and `TALARIA_AGENT_KEY_<SLUG>`.

## Known gap

There is no revocation UI: `rotateAgentApiKey` exists and has no caller, so a leaked credential has
to be rotated by hand (rotate → render → roll). Wiring it to the agents console is the follow-up.

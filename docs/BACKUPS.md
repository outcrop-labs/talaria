# Backups

What a snapshot contains, how to take one on a schedule, and — the part that
actually matters — how to put it back.

Two scripts, no daemon:

```
./scripts/backup.sh [dest-dir]                    # take a snapshot
./scripts/restore.sh <snapshot-dir> [--target …]  # put one back
```

Neither schedules itself. Point cron or a systemd timer at `backup.sh`; the
in-app scheduler (`server/scheduler.ts`) is a later milestone and will call the
same script rather than reimplement it.

## What's in a snapshot

One directory per run, named for the UTC minute it started (`20260731T184421Z`):

| File | What |
|---|---|
| `db.sql.gz` | `pg_dump` of the whole database — plain SQL, `--clean --if-exists --no-owner --no-privileges` |
| `uploads.tar.gz` | every upload blob, flat, named `<upload-id><ext>` |
| `manifest.txt` | when, from which database, which storage mode + bucket. Identifiers only, never credentials |
| `SHA256SUMS` | checksums of the three above; `restore.sh` verifies them before touching anything |

The blob archive has the **same shape in every storage mode** — the filename is
both what `uploads.ts` writes on local disk and the tail of every bucket key. So
a snapshot taken from local disk restores into a bucket and vice versa.

Everything is written to `<stamp>.partial` and renamed only when the run
succeeds, so a half-written snapshot never looks complete and retention can
never delete a good one in favour of a broken one.

## What is deliberately not backed up

- **Redis** — sessions and ephemeral state. Losing it signs everyone out; that's
  the entire blast radius.
- **Qdrant** — every vector is re-derivable from Postgres by reindexing
  (Admin → Retrieval).
- **`fleet/`** — rendered agent configs and compose files, regenerated from the
  database by `POST /api/fleet/render`. That includes `fleet/.env`: every agent's
  `tak_` credential is kept sealed in `agent_keys` (which *is* in the dump), and
  the renderer **rewrites** each `TALARIA_AGENT_KEY_<SLUG>` line from the database
  rather than skipping one that already exists — so a restore against a preserved
  `.env` converges instead of leaving containers presenting dead secrets. Render
  after a restore, then roll any agent whose credential changed
  ([AGENT-KEY-MIGRATION.md](./AGENT-KEY-MIGRATION.md)).
- **`TALARIA_SECRET_KEY`** — and this one needs saying out loud:

> **The root secret is not in the backup, and the backup is useless without it.**
> Provider API keys, Google refresh tokens and per-agent secrets are in the dump
> as ciphertext, and the data keys that decrypt them (`secret_keys`) are in there
> *wrapped by a key derived from `TALARIA_SECRET_KEY`*, which lives only in
> `ui/.env`. Restore a database with a different root secret and every stored
> credential is unrecoverable — see [ENCRYPTION.md](./ENCRYPTION.md). Keep the
> root secret somewhere a snapshot isn't: a password manager, a sealed envelope,
> your secrets store.

## Requirements

`pg_dump` and `psql` on `PATH`, **or** Docker — the scripts borrow the clients
from a throwaway `postgres:16-alpine` container on the host network when the
host has none. Bucket storage additionally needs `mc` or Docker
(`minio/mc:latest`). Override the images with `TALARIA_PG_IMAGE` /
`TALARIA_MC_IMAGE`; bump the Postgres one together with the server, since
`pg_dump` refuses a server newer than itself.

The container fallback uses `--network host`, which is a Linux thing. On
macOS/Windows install the Postgres client instead.

## Storage modes

`backup.sh` reads `app_settings.storage_config` — the same row Admin → Storage
writes — and handles whichever mode is configured:

| Mode | Where the blobs are | What the script needs |
|---|---|---|
| `local` | `TALARIA_UPLOADS_DIR` (default `ui/.uploads`) | nothing |
| `internal` | the bundled MinIO container | `TALARIA_S3_*` from `ui/.env` — the same values the app uses |
| `s3` | your external bucket | endpoint/bucket/key come from the database; **the secret key must be supplied** |

That last row is deliberate. An external bucket's secret is *sealed* in
`app_settings` (secretbox), and a backup must not need the app's decryption keys
to run — otherwise the backup path becomes a second way to unwrap every secret
you own. So hand it over out of band:

```sh
export TALARIA_BACKUP_S3_ACCESS_KEY=…   # restore only; backup reads it from the DB
export TALARIA_BACKUP_S3_SECRET_KEY=…
```

## Configuration

Every knob is an environment variable; a value already exported always beats
`ui/.env`, so one checkout can back up a remote instance.

| Variable | Default | Meaning |
|---|---|---|
| `TALARIA_BACKUP_DIR` | `backups` (or `$1`) | where snapshots land. Created with a self-ignoring `.gitignore` |
| `TALARIA_BACKUP_KEEP` | `7` | how many snapshots to keep; `0` disables pruning |
| `TALARIA_ENV_FILE` | `ui/.env` | which env file to read `DATABASE_URL` etc. from |
| `TALARIA_BACKUP_S3_ACCESS_KEY` / `_SECRET_KEY` | — | external-bucket credentials (see above) |
| `TALARIA_BACKUP_S3_ENDPOINT` / `_BUCKET` / `_PREFIX` | the manifest's | restore-time overrides — send a restore to a drill bucket or a new provider |

Retention only ever removes directories that carry a `manifest.txt`; anything
else you keep in the backup folder is left alone.

## Scheduling

Daily at 03:15, keeping a fortnight:

```cron
15 3 * * *  cd /srv/talaria && TALARIA_BACKUP_KEEP=14 ./scripts/backup.sh /var/backups/talaria >>/var/log/talaria-backup.log 2>&1
```

`backup.sh` exits non-zero on any failure — including a dump that lost its
connection half way, which is checked for explicitly — so a cron MAILTO or a
systemd `OnFailure=` is a real alert.

**The local copy is not the backup.** Copy the snapshot directory somewhere the
machine can't reach (`rclone`/`rsync` to a different provider, or object-lock on
the bucket you sync into). This script gives you the artifact; getting it
offsite is still yours.

## Restoring

Restore is destructive: the dump drops and recreates every object it owns.

1. **Stop the app.** It runs migrations on its first query and holds a pool
   open; neither survives the schema being swapped underneath it.
2. **Point at the right database.** `--target` beats `DATABASE_URL`, which beats
   `ui/.env`. Restoring into a *new, empty* database and repointing the app is
   always safer than restoring over a live one.
3. **Run it.**

   ```sh
   ./scripts/restore.sh backups/20260731T184421Z --target postgres://talaria:talaria@127.0.0.1:5544/talaria
   ```

   It verifies `SHA256SUMS`, prints the target, and asks you to type `restore`
   (`--yes` for automation; it refuses to run non-interactively without it).
   `--db-only` / `--uploads-only` restore one half.
4. **Start the app.** Migrations pick up from wherever `schema_migrations` left
   off in the restored dump, so an older snapshot forward-migrates on boot.
5. **Verify by using it.** Sign in and open a message attachment — one click
   exercises both halves: the row in Postgres and the bytes in storage.

Blobs go back to *this* instance's configured location, not the source's: local
mode extracts into `TALARIA_UPLOADS_DIR`, bucket modes mirror back up to the
bucket named in the manifest (or the `TALARIA_BACKUP_S3_*` overrides).

## The drill

A backup nobody has restored is not a backup. This one takes five minutes and
touches nothing live — it restores into a throwaway container on a port the dev
stack doesn't use:

```sh
# 1. a scratch Postgres, nowhere near the dev one
docker run -d --name talaria-restore-drill -e POSTGRES_USER=talaria \
  -e POSTGRES_PASSWORD=talaria -e POSTGRES_DB=talaria -p 55999:5432 postgres:16-alpine

# 2. restore the newest snapshot's database half into it. --db-only, because the
#    blob half would land in this instance's LIVE storage.
./scripts/restore.sh "$(ls -d backups/*Z | tail -1)" \
  --target postgres://talaria:talaria@127.0.0.1:55999/talaria --db-only --yes

# 3. does it hold the data you expect?
docker exec talaria-restore-drill psql -U talaria -d talaria \
  -c "select count(*) from users" -c "select count(*) from tasks" \
  -c "select max(created_at) from messages"

# 4. throw it away
docker rm -f talaria-restore-drill
```

Step 3 is the whole point: a row count and a recent timestamp tell you the
snapshot is *current*, not merely well-formed. For the blob half, run
`--uploads-only` with `TALARIA_UPLOADS_DIR` set to a scratch directory (local
mode) or `TALARIA_BACKUP_S3_BUCKET` set to a scratch bucket (bucket modes), then
diff the result against the live one.

## Known limits

- **RPO is the interval.** This is a full dump, not WAL archiving — everything
  since the last run is lost in a hard failure. Continuous archiving is a
  separate piece of work if the product ever promises better.
- **The dump is not encrypted.** It contains ciphertext for secrets but plain
  rows for everything else. Encrypt it wherever you store it offsite.
- **The connection string is an argument** to `pg_dump`/`psql`, so it's briefly
  visible in the process list. On a shared host, use a `.pgpass`-style URL
  without an inline password.
- **Large buckets need transient disk** equal to the blob total: bucket modes
  mirror to the staging directory before archiving it.

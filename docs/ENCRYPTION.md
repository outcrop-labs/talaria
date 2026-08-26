# Encryption

How Talaria protects secrets at rest, and the one rule that keeps them
recoverable.

> **Back up the root secret with the database.** `TALARIA_SECRET_KEY` lives only
> in `ui/.env`; a dump restored without it restores an instance that cannot read
> its own secrets. Keep it somewhere a snapshot isn't. ([`BACKUPS.md`](./BACKUPS.md))

## What's encrypted

Live credentials Talaria must store and replay — provider API keys, per-agent
secrets and credentials, Google tokens (per-user and org), MCP OAuth tokens and
headers, the email transport credential, external object-storage keys, the
GitHub App key or PAT, and the reranker key.

**Do not maintain that list by hand anywhere.** `secretHealth()` in
`server/secret-health.ts` enumerates every store, and **Admin → Secrets** shows
what this instance actually holds, per row, with whether it can still be
decrypted. A list in a document goes stale the first time someone adds a
secret; that page cannot.

When you introduce a new sealed value, add it in two places: `CIPHER_TARGETS` in
`secret-rotation.ts` (so rotation re-encrypts it) and `secretHealth()` (so it is
visible and recoverable). `clearSecret()` will not compile a new id form without
you deciding what clearing it means.

Everything is **AES-256-GCM**. That's the post-quantum-safe choice for data at
rest: the only quantum threat to symmetric crypto is Grover's algorithm, which
merely halves the effective key size, so AES-256 keeps ~128-bit security (NIST
treats it as quantum-resistant). Talaria uses **no asymmetric crypto** anywhere —
sessions are random opaque tokens, not signed JWTs — so Shor's algorithm has
nothing to attack.

## Envelope encryption (KEK → DEK)

Two layers, so the key that actually unlocks everything never lives in a config:

- **KEK (key-encryption key)** — derived with scrypt from the **root secret**:
  `TALARIA_SECRET_KEY` (preferred), or `TALARIA_SECRET_KEY_FILE`'s contents, or
  `AUTH_SECRET` as a fallback. This is the *only* key material outside the DB.
- **DEK (data-encryption key)** — a random 256-bit key that encrypts every
  secret. It's stored in the `secret_keys` table **wrapped by the KEK**. So the
  DEK — the thing that decrypts all your secrets — is never in a file.

`secretbox.ts` loads the DEK(s) once during DB migration, so `seal()`/`open()`
are synchronous everywhere else. The in-memory keys live on `globalThis` so a
dev HMR reload doesn't lose them.

## Versioned data keys

Ciphertext is self-describing so multiple app instances and key rotations are
safe:

```
v1:<iv>:<tag>:<data>          # KEK-direct (also how a wrapped DEK is stored)
v2:<iv>:<tag>:<data>          # DEK, legacy unversioned (still readable)
v2:<version>:<iv>:<tag>:<data>   # DEK version <version>  ← what seal() writes
```

Every current token names the **DEK version** that sealed it, and **all versions
are kept** in `secret_keys` forever. So `open()` always decrypts with the exact
key a token was sealed with. A process that doesn't hold a version fails *soft*
(throws → the caller falls back to an env var / config) rather than decrypting
with the wrong key or re-sealing and corrupting it. This is what makes a
second app instance (a dev worktree) safe.

## Rotation

**Admin → Encryption** rotates in one click. It:

1. generates a fresh DEK at the next version,
2. re-encrypts **every** secret (all tables above) under it in one transaction,
3. keeps the prior versions (their ciphertext still decrypts), and
4. optionally re-derives the KEK from a new root secret and re-wraps every
   retained version under it.

`rotateSecrets()` in `secret-rotation.ts` drives it; `CIPHER_TARGETS` lists every
encrypted column — **add new ones there** when you introduce a new secret.

## The one rule: keep the root secret STABLE

The KEK comes from `TALARIA_SECRET_KEY` (or `AUTH_SECRET`). **If it changes, the
wrapped DEKs can't be unwrapped and every stored secret becomes unrecoverable.**

- `talaria setup` generates a **dedicated, stable `TALARIA_SECRET_KEY`** (separate from
  the session-signing `AUTH_SECRET`, which is safe to rotate). Keep it constant.
- Worktrees **copy** this value (`talaria worktree`) so a second stack can
  decrypt data seeded from the first. Never let a worktree mint its own.
- To move the root secret intentionally, use the rotation flow with a new root —
  it re-wraps everything so nothing is lost.

## When the root secret is gone

The app still boots. That is deliberate and was once not true: `initSecretbox`
runs inside the migration pass, so throwing from it rejected every `db()` call
and took down boards, teams and agents — none of which touch a secret. It now
**records** the failure and lets the operations that actually need a key report
it. A loud `[secretbox]` line is logged either way, and unreadable ciphertext
falls back to its env-var source where one exists.

Recovery, in order of preference:

1. **Restore the original root secret.** This recovers everything, and is why
   the backup rule at the top of this document matters.
2. **Admin → Secrets** — per-row health and a per-row **Clear**, plus **Clear
   all unreadable**, which names every value it will delete and leaves readable
   ones alone. This is the normal path: an instance whose Google token predates
   a key change but whose provider key was entered yesterday keeps the second.
3. **`bun talaria reset secrets`** — the backstop for an instance that will not
   start at all, where the UI cannot help. It clears *everything* sealed,
   because the CLI cannot tell what is broken.

After a clear, re-enter what was lost: provider keys on `/models`, Google and
MCP accounts by reconnecting, and re-render the fleet so agents get fresh
credentials.

## Provider keys specifically

Provider API keys live **encrypted in the DB**, entered on `/models` — never in a
config file. `resolveEndpointKey(ep)` reads the sealed key first; an env var /
`fleet/.env` named by the endpoint's `api_key_env` is a fallback for ops
overrides and pre-migration installs. `migrateEnvKeysToCipher()` seals any
config-only key into the DB automatically the first time you open `/models`.

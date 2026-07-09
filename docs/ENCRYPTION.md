# Encryption

How Talaria protects secrets at rest, and the one rule that keeps them
recoverable.

## What's encrypted

Live credentials Talaria must store and replay, in Postgres:

| Where | What |
|---|---|
| `llm_endpoints.api_key_cipher` | provider API keys (OpenAI, Anthropic, OpenRouter, your self-hosted gateway…) |
| `google_connections`, `google_org_connection` | Google OAuth access + refresh tokens |
| `agent_secrets.value_enc` | per-agent secrets (tokens the agent's tools use) |

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

- `setup.sh` generates a **dedicated, stable `TALARIA_SECRET_KEY`** (separate from
  the session-signing `AUTH_SECRET`, which is safe to rotate). Keep it constant.
- Worktrees **copy** this value (`scripts/worktree.sh`) so a second stack can
  decrypt data seeded from the first. Never let a worktree mint its own.
- To move the root secret intentionally, use the rotation flow with a new root —
  it re-wraps everything so nothing is lost.

If the root secret is genuinely lost or was changed without a rotation, the app
still boots (a loud `[secretbox]` error is logged; unreadable ciphers fall back
to their env-var source where one exists). Recovery is: restore the original
root secret, **or** reset the encryption (clear `secret_keys` + the cipher
columns, restart to mint a fresh DEK, and re-enter secrets on `/models`).

## Provider keys specifically

Provider API keys live **encrypted in the DB**, entered on `/models` — never in a
config file. `resolveEndpointKey(ep)` reads the sealed key first; an env var /
`fleet/.env` named by the endpoint's `api_key_env` is a fallback for ops
overrides and pre-migration installs. `migrateEnvKeysToCipher()` seals any
config-only key into the DB automatically the first time you open `/models`.

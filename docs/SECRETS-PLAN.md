# Secrets: inventory, health, and recovery

_Plan, 2026-08-05. Written after a contributor's install spent a day unusable because its
data key could not be unwrapped and there was no way out that did not involve a shell._

> **Status: delivered 2026-08-05.** All six pieces shipped. The two open questions that
> needed answers before building were decided as proposed — admins see per-user rows as
> existence, owner and health only (`USER_SCOPED_METADATA` in `secret-health.ts` records
> it), and `Replace` deep-links rather than duplicating seven forms. The third —
> auto-clearing an unreadable row on replace — was dropped: replacing overwrites the
> ciphertext anyway, so there is no dead weight to collect and no delete to confirm.

---

## Why

Talaria holds nine kinds of secret across seven surfaces, and an operator has no single
place to see any of them:

| what | entered at | stored in |
|---|---|---|
| Provider API keys | `/models` | `llm_endpoints.api_key_cipher` |
| Agent secrets | agent manage modal | `agent_secrets.value_enc` |
| Agent credentials | fleet render | `agent_keys.key_enc` |
| Per-user Google | Settings | `google_connections.*_token_enc` |
| Org Google | Admin → Organization | `google_org_connection.*_token_enc` |
| MCP OAuth tokens | `/mcp` | `mcp_oauth_tokens.tokens_enc` |
| MCP headers | `/mcp` | `mcp_user_credentials.headers_enc` |
| Email transport | Admin → Email | `app_settings.email_config` |
| Object storage | Admin → Storage | `app_settings.storage_config` |
| GitHub App/PAT | Workbench | `app_settings.github_config` |

Three things follow from that spread, and all three bit this week:

**Nobody can answer "what does this instance hold?"** Not the operator, not us when helping
them. Diagnosing the incident meant querying nine tables by hand.

**Recovery is all-or-nothing.** `talaria reset secrets` destroys every sealed value
because it has no way to know which ones are actually broken. An instance whose Google token
predates a key change, but whose provider key was entered yesterday, loses both.

**The failure lands where it is least survivable.** The one thing a new operator must do —
enter a provider key so the product works at all — is itself a secret write. When the data
key is unreadable, that write fails, and the app is unusable with a message about restoring
a root secret they have never heard of. There is no in-app way out. That is the actual bug.

---

## Principles

1. **The inventory is a VIEW, never a second store.** A provider key entered on `/models`
   keeps writing to `llm_endpoints` exactly as it does today, and *appears* in the
   inventory. One value, many entry points, one place to see them. A parallel store would
   reintroduce the "one answer, two copies" problem the rest of this codebase has spent
   considerable effort removing — and `check-invariants.mjs` exists to stop.
2. **Never reveal, only replace.** A sealed value cannot be displayed, and should not be
   even if it could. The inventory shows presence and metadata; the actions are *replace*
   and *remove*. There is no reveal affordance to add later.
3. **Health is per row.** One unreadable Google token is one broken row, not a reason to
   destroy a working provider key. This is the difference between "clear these three" and
   "lose everything you have."
4. **Nothing blocks onboarding.** No setup gate, no wall. You meet each requirement at the
   moment it actually bites, with the fix inline. A missing provider key should stop you
   *chatting*, not stop you *using the app*.
5. **One health probe.** The banner, the bumps and the inventory all read the same function.
   Three implementations of "is this secret okay" is how this codebase historically grows a
   fourth that disagrees.

---

## The pieces

### 1. `secretHealth()` — the one probe

New in `ui/src/server/secret-health.ts`. Returns, per secret: a stable id, what it is, what
it unlocks, where it was set, `updated_at` (every store has it) plus `created_at`,
`last_used_at` or `access_expires_at` where they exist, and a state:

- `ok` — sealed and readable
- `unreadable` — sealed under a data key this process cannot unwrap
- `missing` — not configured
- `env` — lives in the environment, not the database (see below)

Two ways to judge a row, both worth having. **Cheap:** a `v2:<ver>:…` token names its own
DEK version, so "is that version loaded?" answers most rows without touching crypto.
**Certain:** `open()` in a try/catch, because a version can be present and still wrong.
Use cheap for the list, certain for a single row on demand.

**The encryption root is `env`, not a row.** `TALARIA_SECRET_KEY` lives in the process
environment; the app can report its health — set, absent, or serving from the `AUTH_SECRET`
fallback — and must not offer to change it. Saying so plainly in the UI is most of the value:
today nothing tells an operator this value exists until it is too late.

### 2. Admin → Secrets — the inventory

New route + panel. Grouped by what they unlock (models · integrations · agents · platform),
one row each: what it is, what breaks without it, when it was set, when last used or when it
expires, and its state.

Per row: **Replace** (deep-links to the surface that owns it, or takes the value inline where
that is simple) and **Remove**.

An `unreadable` row gets **Clear this** — which deletes that ciphertext and nothing else,
naming exactly what stops working and what has to be re-entered. When several are unreadable,
a **Clear all unreadable** that lists them first. Same guarantees as `talaria reset secrets` —
live counts, explicit confirmation — without a terminal.

### 3. Bumps — non-blocking, contextual

- **No provider configured** and the user opens chat, Plan, or Research → inline "add a key
  to start", with the field present. Not a redirect.
- **No email transport** and the user opens invites, or turns on notification delivery →
  inline nudge with the link.
- **Unreadable secrets exist** → a dismissible banner, admin-only, pointing at Admin →
  Secrets with the count. Dismissible: it is a state to fix, not a modal to fight.
- **Fresh instance, nothing configured** → the provider bump does the work. No setup gate.

### 4. Setup and self-host hardening

Smaller, but it is where the incident started.

- `setup.sh` should verify what it generated actually boots, rather than assuming.
- The relationship between `AUTH_SECRET` and `TALARIA_SECRET_KEY` is now documented in
  `.env.example` and pinned by `setup.sh` — check that a self-hoster following `README`
  rather than the script reaches the same place.
- `docs/ENCRYPTION.md` should state the backup rule in one line: **the root secret belongs
  with the database backup**; a dump restored without it restores an instance that cannot
  read its own secrets.

`talaria reset secrets` stays. It is the right tool when the app will not start at all, and
the UI cannot help there. The UI becomes the primary path; the reset becomes the backstop.

---

## Sequencing

| | piece | why this order | size |
|---|---|---|---|
| 1 | `secretHealth()` | everything else reads it; building it second means building it twice | S |
| 2 | Admin → Secrets, read-only | the diagnosis problem, solved on its own | M |
| 3 | Per-row clear + clear-all-unreadable | recovery without a terminal | S |
| 4 | Replace / remove per row | the management surface proper | M |
| 5 | The bumps | needs the probe; independent of the page | S |
| 6 | Setup + docs hardening | independent, can run in parallel | S |

Steps 1–3 already solve the incident that prompted this. Steps 4–5 are what make it a
feature rather than a recovery tool.

---

## Deliberately out of scope

- **Rotation.** Re-wrapping every secret under a new root is a real feature with real
  failure modes and belongs on its own. `secret-rotation.ts` exists; this plan does not
  touch it.
- **Revealing values.** Not now, not later. See principle 2.
- **A setup wizard.** Rejected deliberately: it replaces one wall with a nicer wall. Bumps
  where the requirement bites, not a gate before the product.
- **Per-user secret management.** This is the operator's inventory. A user's own Google
  connection stays in Settings; it appears here as a row an admin can see the health of and
  clear, not one they can use.

---

## Open questions

- **Does an admin see per-user rows in full?** They can already see that a connection
  exists, and they need to for recovery — but "Jon's Google token" is closer to his than to
  the instance's. Proposal: admins see existence, health and the ability to clear; never
  metadata beyond that, and never the value. Worth a decision before step 2.
- **Does `Replace` take values inline, or deep-link?** Inline is better UX and duplicates
  form logic that already exists on seven surfaces. Proposal: deep-link first, inline only
  where the owning surface is a single field.
- **Should an `unreadable` row auto-clear on replace?** Replacing the value makes the old
  ciphertext dead weight. Probably yes, silently — but it is a delete, so it should be
  stated in the confirmation rather than assumed.

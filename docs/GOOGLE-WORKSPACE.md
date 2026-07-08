# Google Workspace integration

Per-user OAuth: each person connects **their own** Google account, and Talaria —
plus the agents working for them — acts *as that user* against Drive/Docs. Files
land in the user's own Drive, governed by Google's own sharing.

**Whose Google an agent uses** (the identity proxy):
- A user's **personal assistant** acts as *its owner* — resolved from
  `agent_defs.owner_user_id` (the agent's unique `model` is what it presents as
  `x-agent-name` over MCP). It never falls back to the org account.
- A **general fleet agent** (no owner) acts as a single **shared org Google
  account** an admin connects under **Admin → Organization Google account**. So
  the whole swarm has one company workspace to build in.

**Org build targets** (Admin → Organization Google account, once connected):
- **Shared Drive / folder ID** — agent-created files land here so they're
  **team-owned** (survive if the bot account is deleted). Blank → the account's
  My Drive. *The org account must be a member of that Shared Drive.*
- **Calendar ID** — org events land on this calendar (blank → `primary`).
- **Send mail as** — a *verified send-as alias* on the org account for outgoing
  mail (blank → the account's own address). This is where the "one account +
  aliases" pattern pays off.

Reading a *different* mailbox than the connected account's own inbox needs
domain-wide delegation (a service account) — not built; that's the future
multi-account setup path.

## What ships today

- **Connect / disconnect** a Google account from **Settings → Connected
  accounts**. Connecting grants *offline* access (a refresh token) so exports
  work later without the user present.
- **Export an artifact → Google Drive** from an artifact's **⋯ menu**:
  - `doc` → native **Google Doc** (markdown converted by Drive)
  - `microsite` → native **Google Doc** (HTML converted by Drive)
  - `sheet` → native **Google Sheet** (CSV converted by Drive)
  - `file` → uploaded to Drive unconverted (original type preserved)
  - The resulting Drive link is stored on the artifact ("Open in Google Drive").
- **Import a Drive file → artifact** from the Artifacts sidebar (**Import from
  Google Drive**): search your Drive, pick a file:
  - Google **Doc** → `doc` artifact (exported as markdown)
  - Google **Sheet** → `sheet` artifact (exported as CSV → grid)
  - Other native types (Slides, …) → `file` artifact (exported as PDF)
  - Regular files → `file` artifact (downloaded, ≤25 MB)
  - The source Drive link is remembered on the artifact.
- **Calendar agenda** on Home — when connected, an **Agenda** panel lists the
  user's upcoming Google Calendar events with a quick **New event** create
  (title + start; defaults to a 1-hour event). Hidden entirely when not
  connected, so Home stays clean.
- **Mail** on Home — when connected, a **Mail** panel shows recent Gmail
  (sender / subject / snippet, unread bolded, link out to Gmail) with a
  **Compose** modal that sends plain-text email as the user. Also hidden until
  connected.
- **Agents** export via the `export_to_google_doc` MCP tool — into their owner's
  Drive (personal assistant) or the shared org Drive (general agent). See the
  identity-proxy note above.
- **Agent Calendar & Gmail (confirm-sends):** an agent can `read_calendar` /
  `read_recent_email` freely, but `draft_calendar_event` / `draft_email` do
  **not** act immediately — they queue a **pending action** a human approves on
  Home (**Needs your approval**). On approve it runs; reject drops it.
  - A **personal assistant** reads/drafts on **its owner's** Google — the
    **owner** approves.
  - A **general agent** reads/drafts on the **shared org** Google — an **admin**
    approves (org actions are tagged `org` in the approvals panel).

## Security

- Refresh tokens are **encrypted at rest** (AES-256-GCM, `server/secretbox.ts`,
  key derived from `AUTH_SECRET`). They are the one runtime credential we must
  store — unlike API keys, where we persist only the env-var *name*.
- Access tokens are cached (also encrypted) and refreshed transparently; an
  `invalid_grant` clears the connection so the UI prompts a reconnect.
- Scopes: **`drive.file`** (create/manage only files the app makes — export),
  **`drive.readonly`** (browse + read for import), **`calendar.events`** (agenda
  + create), **`gmail.readonly`** (recent mail) + **`gmail.send`** (send).
  Adding scopes over time means existing users must **reconnect** to grant the
  new ones; each surface detects a missing scope and prompts a reconnect.
  `gmail.*` are **restricted** scopes — fine for an internal/Workspace-only app,
  but public/external distribution would need Google's security assessment.
- Disconnect best-effort **revokes** the token at Google, then forgets it.

## Operator setup

Uses the **same** Google OAuth client as Google login:

```
AUTH_GOOGLE_ENABLED=1
AUTH_GOOGLE_CLIENT_ID=<client id>.apps.googleusercontent.com
AUTH_GOOGLE_CLIENT_SECRET=<client secret>
AUTH_SECRET=<random, ≥32 bytes>        # also encrypts stored tokens
# AUTH_GOOGLE_HD=yourdomain.com        # optional: restrict to a Workspace domain
# TALARIA_SECRET_KEY=<override>         # optional: separate key for secretbox
```

In the **Google Cloud console** for that OAuth client, register **both** redirect
URIs (login + connect):

```
<public-url>/api/auth/google/callback
<public-url>/api/integrations/google/callback
```

and enable the **Google Drive API**, **Google Calendar API**, and **Gmail API**.
Add the `drive.file`, `drive.readonly`, `calendar.events`, `gmail.readonly`, and
`gmail.send` scopes to the consent screen. `drive.file` is not restricted;
`drive.readonly` / `calendar.events` are sensitive; `gmail.*` are restricted.
For an **Internal** (Workspace-only) OAuth app none of these need a verification
review; a public/External app would (especially the Gmail scopes).

## Verified vs. trusted

The connect redirect (offline + `drive.file` + `prompt=consent`), the token
encryption round-trip/tamper-rejection, the DB model, and the export permission
gating are all verified live. The **actual Drive round-trip** (create Doc/Sheet)
requires real Google client credentials + a consented account and cannot be
exercised in a headless dev env — it's wired to Drive's documented multipart
convert-on-upload API but is the one path proven by the API contract rather than
a local screenshot.

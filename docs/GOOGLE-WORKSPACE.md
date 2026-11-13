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
  Home (**Needs your approval**). The draft call is not proof it landed: the
  agent confirms with `list_pending_sends` (the same queue) before saying it
  is ready. On approve it runs; reject drops it. An approval is **claimed**
  before it executes (`status = 'executing'`), so two people — or one person
  on the chip and the inbox card — approving the same row inside the Google
  round-trip send once; the second sees it already decided.

  - A **personal assistant** reads/drafts on **its owner's** Google — the
    **owner** approves.
  - A **general agent** reads/drafts on the **shared org** Google — an **admin**
    approves (org actions are tagged `org` in the approvals panel).
  - **The unlock an approval leaves is bounded.** Approving a chip in a chat
    writes a grant on that conversation (`conversations.unlocked_tools`, one
    `{tool, expiresAt, usesLeft, recipients}` object per tool): it lasts
    **30 minutes**, covers **one** further call of that tool, and — for
    `gmail_send` and `calendar_create` — covers only a draft whose
    recipients (to/cc/bcc) or attendees are a **subset** of the ones the
    approved draft named. The next `draft_email` / `draft_calendar_event`
    from the personal assistant in that conversation spends the grant (one
    use, under a row lock) and executes, answering *"Sent — the earlier
    approval covered this one; the next needs its own."*; anything outside
    the grant — a new recipient, a second call, half an hour later — queues a
    fresh card. Org drafts never ride an unlock.
  - **Pending lists are per agent.** `list_pending_sends` /
    `read_pending_send` show an org agent only the rows it queued itself,
    never another agent's unsent mail; the admin's queue still shows all of
    them.
- **Agent Docs & Drive (what is immediate, what queues):** `create_google_doc`
  and `create_google_folder` are immediate in the account's own Drive or
  inside a folder the agent created; inside **any other folder** they queue
  (`doc_create` / `drive_folder_create`) because the file inherits that
  folder's sharing. `update_google_doc` on a doc the agent created is
  immediate until **a person edits it**, read two ways before every
  full-body replace: Drive's `lastModifyingUser` is not the token holder
  (someone else), or Drive's `modifiedTime` is later than Talaria's own
  stamp of the agent's last write (`last_agent_write_at`, taken on every
  agent write — which catches the owner editing their assistant's doc from
  their own account, the one case the `me` flag cannot tell from the agent).
  From then on a full-body replace queues like any human-owned doc, and
  `append_google_doc` stays immediate. `read_drive_file`
  and `import_drive_file` stop at the upload limit (25 MiB).
- **Who an agent may act as.** Setting an agent's Google principal to
  `owner` for a user **other than yourself** is admin-only (an agent's
  manager can bind it to their own account); the person it names is notified
  either way, with who did it. Connecting the org account or an agent's own
  account writes an audit row (`google.org_connect`, `agent.google_connect`)
  and records who connected it.

## Security

- Refresh tokens are **encrypted at rest** (AES-256-GCM, `server/secretbox.ts`,
  key derived from `AUTH_SECRET`). They are the one runtime credential we must
  store — unlike API keys, where we persist only the env-var *name*.
- Access tokens are cached (also encrypted) and refreshed transparently; an
  `invalid_grant` clears the connection so the UI prompts a reconnect.
- Scopes actually requested (`WORKSPACE_SCOPES` in `talaria-google-oauth`):
  **`openid`**, **`email`**, full **`drive`** (browse, import, rename, move,
  trash, create — `drive.file` / `drive.readonly` could not manage the Drive
  the Files place browses), **`calendar.events`** (agenda + create),
  **`gmail.modify`** (read **and** organize: labels, mark-read, archive) and
  **`gmail.send`** (send). The connect URL sets `include_granted_scopes=true`,
  so a token carries every scope that Google identity granted the app before.
  The **org** connect asks for more (`ORG_CONNECT_SCOPES`): full
  **`calendar`** and full **`drive`**, which provisioning needs to create the
  shared calendar and Shared Drive and their domain shares. Adding scopes over
  time means existing users must **reconnect** to grant the new ones; each
  surface detects a missing scope and prompts a reconnect. `gmail.*` are
  **restricted** scopes and full `drive` is **sensitive** — fine for an
  internal/Workspace-only app, but public/external distribution would need
  Google's security assessment.
- Disconnect best-effort **revokes** the token at Google, then forgets it.

## Operator setup

### Two client roles

Talaria gives a Google OAuth client one of two jobs. Admin → Google Workspace
has a card for each, and each has an env twin:

| Role | What runs on it | Admin record | Env fallback |
|---|---|---|---|
| **Connections** | every stored grant: personal connect, the org account, per-agent identities. Also Google login while no sign-in client is set. | `google_oauth_client` | `AUTH_GOOGLE_CLIENT_ID` / `_SECRET` / `AUTH_GOOGLE_HD` |
| **Sign-in** (optional) | Google login only | `google_signin_oauth_client` | `AUTH_GOOGLE_SIGNIN_CLIENT_ID` / `_SECRET` |

- A record wins over its env twin only when it is complete (id and secret).
  A half-saved record never takes over.
- With the sign-in slot empty, login uses the connections client. That is how
  every install ran before the split, and nothing changes until a sign-in
  client is saved.
- **The domain restriction (`hd`) belongs to the connections client.** Login
  sends and enforces the connections client's domain whichever client runs it,
  so setting a sign-in client can never open login to every Google account.
  Login admits any Google account only when a connections client is configured
  with no domain, the same configuration that did before the split. With no
  connections client at all, the sign-in client runs login only if a domain is
  still written down (`AUTH_GOOGLE_HD`, or a connections record saved without
  its secret). So removing the connections client stops login, as it always
  did, and the panel says why.
- Moving login to a sign-in client changes no stored connection: every
  grant keeps refreshing with the connections client, which minted it.

**Why two.** Google now sorts OAuth clients into agentic and not, with the
checkbox **"Use this client for an AI-powered agent"** under *Google Auth
Platform → Clients*. Google's notice: "Starting November 11, 2026, Google
Workspace MCP servers will reject requests from any app or integration whose
OAuth client is not marked as agentic … OAuth clients that are not marked as
agentic by November 12, 2026 will remain non-agentic and can't call Google
Workspace MCP servers." And: "If an existing OAuth client is used for both AI
agent and non-agent features, create a separate OAuth client for the AI agent
and mark only that client as agentic. Keep the existing client for your
non-agent features." The REST APIs Talaria calls today are not affected: "Apps
and integrations that are not AI agents, like those that only call Google
Workspace REST APIs, are not affected by this change."

**Which shape to run.** Signing in is a person, not an agent:

- **The connections client is marked agentic** (it was ticked before the
  lock, so every stored grant can reach Google's MCP servers later with no
  reconnect). Create a second Web client *without* the box ticked, register
  only `<public-url>/api/auth/google/callback` on it, and save it as the
  sign-in client. Then remove the login callback from the connections
  client's redirect URIs.
- **The connections client is not marked, and was not by November 12.** It
  stays non-agentic. Nothing breaks: the REST surfaces keep working, and
  login can stay on it. Reaching Google's MCP servers will need a new agentic
  client. **Do not swap a new client into the connections slot while
  connections are stored:** today every stored grant refreshes with whichever
  client is in that slot, and OAuth binds a refresh token to the client it
  was issued to (RFC 6749 §6). Grants that remember their own client are
  separate, later work.

**Declaring the mark.** Google exposes no API and no token claim for the
checkbox, so each card asks what Google Cloud shows: *Ticked*, *Unticked* or
*Not said*. The declaration is stored per client id (`google_client_evidence`
in `app_settings`), so a client moved between roles keeps it. Headless
installs pin it in env: `AUTH_GOOGLE_CLIENT_AGENTIC` and
`AUTH_GOOGLE_SIGNIN_CLIENT_AGENTIC` (`1` ticked, `0` not), each describing
only the client its own `*_CLIENT_ID` names. A declaration refuses nothing.
The panel warns when the sign-in client is declared agentic, and when login
is on, runs on the connections client, and that client is declared agentic.
It also warns when a sign-in client is set but there is no connections client
and no domain for login to take.

### Credentials and redirect URIs

```
AUTH_GOOGLE_ENABLED=1
AUTH_GOOGLE_CLIENT_ID=<client id>.apps.googleusercontent.com
AUTH_GOOGLE_CLIENT_SECRET=<client secret>
AUTH_SECRET=<random, ≥32 bytes>        # also encrypts stored tokens
# AUTH_GOOGLE_HD=yourdomain.com        # optional: restrict to a Workspace domain
# AUTH_GOOGLE_SIGNIN_CLIENT_ID=<id>    # optional: login's own client
# AUTH_GOOGLE_SIGNIN_CLIENT_SECRET=<secret>
# TALARIA_SECRET_KEY=<override>         # optional: separate key for secretbox
```

In the **Google Cloud console**, register these redirect URIs on the
**connections** client (per-user connect, org connect, per-agent identity):

```
<public-url>/api/integrations/google/callback
<public-url>/api/integrations/google/org/callback
<public-url>/api/integrations/google/agent/callback
```

and the login callback on whichever client runs login: the sign-in client
when one is set, otherwise the connections client:

```
<public-url>/api/auth/google/callback
```

The Admin → Google Workspace panel lists each client's URIs with the actual
origin — copy them from there rather than by hand.

and enable the **Google Drive API**, **Google Calendar API**, and **Gmail API**
(listed with console links under Admin → Google Workspace · OAuth client;
**Test** next to the connected org account probes all three live — consent
succeeds even with the APIs off, so the probe is how you find out). Add the
`drive`, `calendar`, `calendar.events`, `gmail.modify`, and `gmail.send`
scopes to the consent screen. `drive` / `calendar` / `calendar.events` are
sensitive; `gmail.*` are restricted.
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

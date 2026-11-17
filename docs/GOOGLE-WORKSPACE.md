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

- Refresh tokens are **encrypted at rest** (AES-256-GCM envelope encryption,
  `api/crates/talaria-secretbox`: a data key per version, wrapped by a key
  derived from the root secret `TALARIA_SECRET_KEY`, falling back to
  `AUTH_SECRET` — [ENCRYPTION.md](./ENCRYPTION.md)). They are the one runtime
  credential we must store. A `talaria reset secrets` clears every Google token
  store: per-user (`google_connections`), org (`google_org_connection`) and
  per-agent (`agent_google_connections`).
- Access tokens are cached (also encrypted) and refreshed transparently,
  with the OAuth client that minted the grant (see [Which client a connection
  refreshes with](#which-client-a-connection-refreshes-with)). An
  `invalid_grant` clears the connection so the UI prompts a reconnect. An
  `invalid_client` or `unauthorized_client` never does: it is the server's
  client at fault, so the connection is kept and flagged with Google's words.
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
- Disconnect — personal, org, or an agent's own identity — **revokes** the
  grant at Google, then forgets the connection. A revoke ends more than the
  token sent — Google: "Revocation removes all OAuth 2.0 scopes previously
  granted to a project, invalidating any issued access or refresh tokens for
  all clients registered under that project" (see below) — so a disconnect
  first checks the other stored connections: when another one holds a live token
  for the same Google account (an admin who connected the org with their own
  account; an agent identity connected with its manager's account), it
  forgets its own row **without revoking** and says so — the last connection
  to go revokes the grant. A revoke Google refuses still forgets the row, and
  the answer quotes Google's words.

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
  grant keeps refreshing with the client that minted it.

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
  client. Create it as a Web client with the agentic box ticked and the three
  connect callbacks, move the old client to the sign-in slot (it keeps login,
  and keeps every grant it minted refreshing), and save the new one as the
  connections client. New connects mint on the new client; each existing
  connection moves to it when its owner reconnects. *Stored connections by
  client* on the connections card counts them per client as you go.

**Declaring the mark.** Google exposes no API and no token claim for the
checkbox, so each card asks what Google Cloud shows: *Ticked*, *Unticked* or
*Not said*. The declaration is stored per client id (`google_client_evidence`
in `app_settings`), so a client moved between roles keeps it. Headless
installs pin it in env: `AUTH_GOOGLE_CLIENT_AGENTIC` and
`AUTH_GOOGLE_SIGNIN_CLIENT_AGENTIC` (`1` ticked, `0` not), each describing
only the client its own `*_CLIENT_ID` names. A declaration refuses no REST
call. The panel warns when the sign-in client is declared agentic, and when login
is on, runs on the connections client, and that client is declared agentic.
It also warns when a sign-in client is set but there is no connections client
and no domain for login to take.

**Checking the mark with Google.** The only provider-side evidence of the mark
is Google's MCP servers accepting a call: Google's notice says they "will
reject requests from any app or integration whose OAuth client is not marked
as agentic". **Check with Google**
(`POST /api/admin/google-client/check?role=connections|signin`, admin only,
audited as `google.client_check`) makes one `tools/call list_labels` to
`https://gmailmcp.googleapis.com/mcp/v1`, one unit of the Gmail MCP server's
query-cost quota, with **your own** Google connection. That connection must
have been made with the client being checked, so the button sits on that
client's card alone (`GET /api/admin/google-client` carries the client as
`personalClientId`), and the other card says why. If you have no connection,
or yours is recorded as made with another client, the route refuses with a
409 that names the client and sends nothing to Google, not even a token
refresh. A connection that has not recorded its client yet is refreshed
first, because the refresh is what records it. A sign-in client that has
only ever run login made no stored connection, so it cannot be checked this
way, and it needs no check: signing in is not an agent. A client that was the connections client
before it moved to the sign-in slot (the "not marked by November 12" shape
above) can be checked through a connection it made then, because a connection
keeps the client that made it. Google's answer is stored under the client's id as
`lastCheck` (`{ok, status, text, at}`) beside the declaration, and the card
shows it:

- `ok` is true only for an HTTP 2xx carrying a JSON-RPC result without
  `isError`. That is Google confirming the mark, and it outranks a
  declaration of *Unticked*. The result itself, your mail labels, is not
  stored.
- Anything else is shown as Google sent it: the HTTP status and Google's
  words, folded and capped, never reworded. A refusal is **not** read as "not
  marked". Google's body for an unmarked client has not been captured, and
  the same call also fails, in Google's words, when the project is not in the
  Workspace Developer Preview, has not enabled `gmailmcp.googleapis.com`, or
  the token lacks a Gmail scope. It is still the last answer: only the last
  check is kept, so a refusal after an acceptance ends the confirmation, and
  the client's standing falls back to its declaration. For example, a token
  Google does not accept answers HTTP 401 `Request had invalid
  authentication credentials. Expected OAuth 2 access token, login cookie or
  other valid authentication credential. See https://developers.google.com/identity/sign-in/web/devconsole-project.`
  (captured 2026-11-17).
- A call that never got Google's answer (a timeout, no network) is a 502
  `google_unreachable`, and nothing is recorded.

### One door for agent tokens

Every agent Google token comes through one function,
`talaria_google_agent::vend_principal_token`. The agent routes
(`resolve_agent_google`), the approval executor, the queue-time write
refusal and the agent record all read through it:

1. **The principal names one stored grant.** `owner` reads that person's own
   connection, `org` reads the org account, and `agent` reads the agent's own
   identity. An owner with no connection gets nothing, never the org
   account's. A caller that is not proven (the legacy shared key) gets
   nothing, and so does a registry read that fails.
2. **The grant refreshes with the client that minted it** (see *Which client
   a connection refreshes with* above). A token that cannot be had is an
   error in Google's words, never "not connected".
3. **The purpose decides whether that client qualifies.** A REST call takes
   any grant, which is today's behaviour for every agent tool. A call bound
   for Google's Workspace MCP servers takes a grant unless its minting client
   is declared *Unticked* and Google's last answer to a check did not accept
   it. Then the token is not handed out for those servers, and the answer
   says that nothing was sent to them and names the client and the fix (the
   refresh in step 2 may already have reached Google's token endpoint): a
   reconnect when the connections slot now holds a client that is not
   declared unticked, else an admin's. A client nobody has declared, or a grant that
   has not recorded its client yet, goes to Google, and Google's answer is
   relayed. Eligibility follows the **minting client**, never the table the
   grant sits in. So when the connections client was flipped to agentic,
   every stored grant already qualifies, with no reconnect.

The Workspace MCP pack (below) is the door's first `HostedMcp` caller. A
separate agent client (full separation) is a later change. The agent record
shows what such a call would do: `GET
/api/fleet/defs/{id}/google` carries `agentic` (`verified`,
`declared_agentic`, `declared_not_agentic`, `unknown`, or null with no
recorded client) and `mcpRefusal`: why such a call would be refused and the
fix, or null. Its `connected`, `email` and `clientId` now describe the org
account's grant for an `org` agent too, as they already did for the other
two kinds. The org account's address and its `reconnectReason` stay
admin-only, as on the org connection's own routes: an agent manager who is
not an admin reads them as null.

### Google's Workspace MCP servers (Developer Preview)

Google runs its own MCP servers for Gmail, Drive, Calendar, Docs, Sheets,
Slides, Chat, People and Universal Search. Talaria can put them behind its MCP
gateway, **off by default and per instance**: reads run, and every write
waits for a person to approve it. They are in Google's **Developer Preview**, whose terms say: "features in Developer
Preview may not be included in public applications prior to the General
Availability announcement." (<https://developers.google.com/workspace/preview>).
Talaria's own Google tools (the `talaria` toolkit's `read_email`,
`draft_email`, `read_calendar` and the rest) are unchanged and stay the
primary surface; nothing is superseded before Google announces GA.

**Turning one on.** Admin → Org → **Google Workspace MCP**. Each product has a
switch; enabling it asks the admin to acknowledge the preview terms
(`POST /api/admin/google/mcp/{product}` with `acknowledgePreview: true`,
audited as `google.mcp_enable`). It creates one registry row,
`google-<product>` with `auth_mode = 'google'`, **assigned to no agent**, and
asks Google for its catalog. Assign it to agents on the MCP page like any
other server; the owner-access and team rules apply unchanged. Disabling
(`DELETE …/{product}`, `google.mcp_disable`) removes the row, its
assignments and its tool overrides; a held call still waiting fails when
approved (its server is gone). The row's address, credentials and auth mode belong to the pack:
the MCP page refuses to change them, and the custom-URL form refuses Google's
Workspace MCP hosts with a sentence naming the pack. Google serves each of
those servers under several names, and every one is refused: any host under
`googleapis.com` or `google.com` with a label ending in `mcp`
(`gmailmcp.googleapis.com`, `gmailmcp.mtls.googleapis.com`,
`gmailmcp.clients6.google.com`, `content-gmailmcp.googleapis.com`, …), and
People's `/mcp` path on any of its names (`people.googleapis.com`,
`people.mtls.googleapis.com`, `content-people.googleapis.com`). Probed with
no credential on 2026-11-17, 49 such names answered `tools/list` with their
server's catalog byte for byte
(`api/crates/talaria-google-mcp/fixtures/alias-hosts.json`), and the guard's
test refuses every one of them. Registered by URL they would take no Google grant, or an
attached one with none of the pack's policy. A row that already names one (it
was registered before this guard) is relayed nothing by the gateway, and
`call_mcp_tool` refuses it too.

**Whose account a call uses.** Each call speaks as the calling agent's own
Google principal, through the one door with `Purpose::HostedMcp`:

- `owner` → that person's own connection; an owner with none is told to
  connect Google in Settings → Connections, and the org account is **never**
  used instead.
- `org` → the org account (Admin → Org).
- `agent` → the agent's own identity (its page).

A legacy shared-key caller is refused outright. A grant minted by a client
declared *Unticked* for "Use this client for an AI-powered agent", with no
acceptance from Google on record, never reaches Google: the agent reads the
door's sentence (the JSON-RPC `-32001` answer the gateway gives a server that
is not connected). A token that cannot be had reads in Google's words. The
bearer is injected by the gateway; refresh and access tokens never enter an
agent's container, a tool result or a log. Calls go to the product's endpoint
as the code names it, never to the row's stored URL.

**Which methods reach Google.** A pack row relays `initialize`, `ping`,
`tools/list`, `tools/call` and `notifications/*` only. Any other method
(resources, prompts, completion, logging, one Google adds later) is answered
by the gateway with `-32601`, and a batch or a JSON-RPC response gets HTTP 400.
None of them is sent to Google. Google receives the JSON the gateway's gate
read, re-serialized, never the agent's raw bytes.

**What runs.** Each tool has a policy: **allow** relays it, **hold** waits for
a person, **deny** never runs. The default, per tool:

| Default | Tools | What happens |
|---|---|---|
| **allow** | the pinned reads, e.g. Gmail `search_threads`, `get_thread`, `list_labels`; Calendar `list_events`, `suggest_time`; Drive `search_files`, `read_file_content`; Docs `read_doc` | relayed to Google |
| **hold** | every write (Gmail `create_draft` and the label tools, Calendar `create_event`/`update_event`/`delete_event`/`respond_to_event`, Drive `create_file`/`copy_file`, Docs/Sheets/Slides updates, Chat `send_message`), and **any tool Google adds later** | queued for a person (below); Google hears nothing until they approve |
| **deny** | trash and untrash, mark and unmark spam, `apply_sensitive_*` | refused, always |

**The same floor, by argument.** Gmail's label tools reach the trash and spam
without those words in their names: Google's own catalog says `unlabel_thread`
and `unlabel_message` take "a system label ID (e.g., `INBOX`, `TRASH`, …)",
and `label_*` and `update_message_labels` take system label IDs too. So a call
of **any** tool that names `TRASH`, `SPAM` or `BIN` (any case) in a label
argument (`labelIds`, `addLabelIds`, `removeLabelIds`, …) is refused at the
gateway, before a hold, an admin's allow or an unlock is read, and again when
an approved call is replayed: nothing is queued and nothing reaches Google.
No override, unlock or approval lifts it. The set is Talaria's REST organize
path's own (`FORBIDDEN_LABELS`). No read tool Google lists takes a label
argument.

A tool is **allowed** only when it is on the pinned read list
(`talaria-google-mcp`'s `policy::READ_LIST`: every tool Google's live catalogs
marked `readOnlyHint: true` on 2026-11-17) **and** the server's cached catalog
marks it `readOnlyHint: true`. Google calls tool annotations untrusted hints,
so neither fact is enough alone. A tool Google's catalog (as last listed) does
not have is refused, not held. A call whose arguments name a workspace secret
(`«secret:…»`) is refused too, held or not: secrets are never sent to Google's
Workspace MCP servers, and never sit in the approval queue. Every refusal says
nothing was sent (the token is vended first, so its refresh may already have
reached Google's token endpoint).

**Per-tool overrides.** Admin → Org → Google Workspace MCP lists every tool
with its default and a selector (`PUT
/api/admin/google/mcp/{product}/tools/{tool}` with `{policy: "allow" | "hold"
| "deny" | null}`, audited as `google.mcp_tool_policy` with before and after;
stored in `mcp_tool_policies`). An admin can let a write run without a person,
hold a read, or deny anything. **An allow never reaches a personal
assistant:** a call into one member's own Google account stays held for that
member, because the approval model gives the owner alone — never an admin —
the say over an owner's row, and an allow would be an admin's standing
approval of every such call. An allow applies to the org account and to
agents' own identities, whose calls an admin approves anyway; hold and deny
apply to every caller. The panel shows both (`effective`, and
`effectivePersonal` where an allow does not reach). **The deny list is a
floor:** the route refuses
any override on a trash, spam or sensitive-label tool but deny, and the
gateway ignores one that got there anyway. An override names one tool on one
server, so a tool Google adds later never inherits one: it starts held. A
change tells the agents holding the server to list its tools again.

**Holds: the `mcp_call` approval.** When an agent calls a held tool, the gateway does not relay it. It queues the
exact call — server, tool, the arguments as sent, and what Google's catalog
said of the tool — in the same confirm-sends queue Talaria's own Google drafts
use (`google_pending_actions`, kind `mcp_call`), and answers the agent with an
ordinary tool result: `Queued for approval as <id>. Not done yet: …`. The
call's record in `mcp_tool_calls` reads `held`. A held tool is listed to the
agent with that said ahead of Google's description, and without Google's
`outputSchema` (its call answers with an approval id, not that shape). The
hold runs after the pack's own refusals and the agent's tool subset, and
before the credential spend boundary; a retried identical call converges on
the row still waiting, or on one whose approval is running it, so one write
never runs twice. A retry of a call whose approval was claimed more than five
minutes ago and never settled (a process stopped mid-replay) is told its
outcome is unknown and to check in Google; nothing new is queued.

Who approves is the principal's, as for every Google action:

| Principal | Approves | Runs as |
|---|---|---|
| `owner` (a personal assistant) | its owner, alone | the owner's own grant |
| `org` | any admin | the org account's grant |
| `agent` (its own identity) | any admin | the agent's own grant |

The row reaches the approver everywhere a drafted email does: a **Google tool
call** card in the conversation the agent is speaking in, the Inbox focus and
the brief (`APPROVAL · GOOGLE TOOL`, with the server, the tool and the
arguments as evidence), the approvals census and its nudges, and the agent's
own `list_pending_sends` / `read_pending_send`.

**Approving replays the call once**, through `decide_action`'s `mcp_call`
arm, after re-checking: the arguments still name no secret; the registry row
still exists, is still the pack's and is turned on; Google's catalog still
lists the tool; the policy (an override, else the default) is not deny now,
and the arguments name no `TRASH` or `SPAM` label; and the agent may still use
the server and the tool — its assignment or team grant, and for a personal
assistant its owner's user and team access, read as the gateway reads them
(`talaria-mcp`'s `mcp_allowance`). Access an admin revoked after the call was
queued fails it, with nothing sent.
Only then does it vend a token for `Purpose::HostedMcp` through the one door
— a grant minted by a client declared unticked never reaches Google — and send
one stateless `tools/call` to the product's endpoint as the code names it.

| What Google answers | The row |
|---|---|
| a completed tool result | `executed`; the result is kept (capped at 32 KiB) |
| a refusal, a JSON-RPC error, or a tool result with `isError` | `failed`, with Google's words quoted |
| HTTP 401 | back to `pending` (nothing ran), Google's words as `lastError`; the cached token is dropped so the next approval refreshes |
| an elicitation (`elicitation/create` or any request from the server) or a multi-round-trip request | `failed`, quoting what Google asked. Talaria answers none until one has been captured live (owner decision D6) |
| no answer, or an answer that is not a completed tool result | `failed`, saying it may or may not have run: approving again could run a write twice |
| no token, or a grant whose client is declared unticked | back to `pending`, with the reconnect or the door's sentence; nothing sent |

A 401 or 403 is also recorded on the grant as `last_mcp_refusal`, as the
gateway records one; an accepted replay forgets a flag.

**One more call without a new card (personal assistants only).** Approving a
held call in a conversation leaves a grant there keyed `<server>/<tool>`
(`google-calendar/create_event`), bound to whoever the approved call addressed
— its Calendar attendees (`attendees`, `addedAttendees`, `attendeeEmails`,
`addedAttendeeEmails`, `removedAttendeeEmails`), its Gmail `to`/`cc`/`bcc`, or
its Chat space (`conversationId`, and the space a `threadId` names) — as
`draft_email`'s grant is bound to its recipients. For thirty minutes, the
agent's next call of that same tool on that same server, addressing no one
the approved call did not, runs at once in the owner's name, and the agent
reads Google's result. A call adding an attendee, a recipient or another
space is held as usual and does not spend the grant; a call naming nobody is
covered. One use, then it is spent; another tool, or the same tool on another
server, is held as usual. An org-account or agent-identity call never rides an
unlock: an admin approves each one.

**What a held call keeps.** The arguments sit in the queue as sent, readable
only through the approver's routes and the agent's own queue reads, as a
drafted email's body does today. They never enter a log or the call record.

**Scopes come from Google.** Each tool's protected-resource document
(`https://<server>/.well-known/oauth-protected-resource/<tool>`, RFC 9728)
lists the scopes Google accepts for it. The catalog refresh stores that list
per tool (`requiredScopesAnyOf`), and a call whose grant holds none of them is
answered before Google is asked: which scopes the tool takes, and whether a
reconnect would grant one. Talaria's personal connect (`drive`,
`calendar.events`, `gmail.modify`, `gmail.send`) covers every pinned read but
Calendar's `list_calendars` (it takes `calendar`, `calendar.readonly` or the
calendar-list scopes; the org connect's `calendar` covers it) and the Chat and
People tools, whose scopes no Talaria connect asks for.

**When Google refuses.** The agent reads Google's words, quoted, as a tool
result with `isError` (an HTTP 401 handed to the agent's own MCP client would
send it looking for an OAuth flow Talaria owns). A 401 or 403 is also stored
on the grant as `last_mcp_refusal` (`{status, text, tool, at, flagged}`), and
the admin panel lists the latest. A 401 drops the cached token and retries
once on a fresh one. A 401 that persists is stored with `flagged: true`:
Settings → Connections asks the person to reconnect, and the admin panel says
a fresh token was refused too. It is not written to `reconnect_reason`, which
is the token endpoint's refusal of the OAuth client: the refresh that minted
the fresh token succeeded, and the cards and the census read
`reconnect_reason` as the client's. The flag stands until Google accepts a
call made with that grant, a later refusal does not repeat it, or a reconnect
brings a new grant. A 403 never flags: it may be one file or one quota. A 429 or 5xx is only quoted. No refusal is matched
for a class: Google's answer to a client not marked for AI agents has not been
captured.

**What the operator does in Google Cloud.**
1. Enrol the Workspace account and the Google Cloud project in the [Google
   Workspace Developer Preview Program](https://developers.google.com/workspace/preview).
2. Enable each product's REST API **and** its MCP service — the panel prints
   the command, e.g. `gcloud services enable gmailmcp.googleapis.com
   gmail.googleapis.com calendarmcp.googleapis.com calendar-json.googleapis.com`.
   People has no MCP service of its own; Universal Search is
   `workspacemcp.googleapis.com`. Chat also needs a Chat app with interactive
   features turned off.
3. Mark the connections client "Use this client for an AI-powered agent", say
   so on its card, and press **Check with Google**.

**What is not proven live.** Nobody here is enrolled in the Developer Preview,
so no authenticated call to Google's Workspace MCP servers has been made. The
catalogs, the per-tool protected-resource documents, the `initialize` answer
and the 401 bodies were captured from Google without a credential on
2026-11-17 (`api/crates/talaria-google-mcp/fixtures/`). An accepted
`tools/call`, a 403, an authenticated `tools/list` (it may differ from the
unauthenticated one), elicitation or MRTR on a read or on an approved replay,
and Google's refusal of an unmarked client are all still unobserved. The
replay's accepted, elicitation and MRTR shapes are the MCP specification's,
and its tests say so.

**Testing against a stand-in.**
`TALARIA_TEST_GOOGLE_MCP_ORIGINS=gmail=http://127.0.0.1:7811,calendar=…` sends
those products' calls to a loopback stub instead of Google. Only a loopback
`http://` origin is taken; anything else in the variable is ignored. The code
is compiled into debug-profile builds only: `talaria dev`'s `cargo run`, CI's
`--api-from-ci` artifact, and also the nightly and rc channel images, which
`release.yml` builds with the dev profile. It is read only outside a
deployment: a process carrying the published image's `TALARIA_INSTALL=image`,
or a production checkout's `TALARIA_RUNTIME=prod-server`, ignores the
variable. A release-profile build (main's feed and every stable tag) has no
such code. The gateway's address guard still applies, so the stack also needs
`TALARIA_FETCH_ALLOW_HOSTS=127.0.0.1`.

### Which client a connection refreshes with

OAuth binds a refresh token to the client it was issued to (RFC 6749 §6).
So every stored connection, whether personal (`google_connections`), the org
account (`google_org_connection`) or an agent identity
(`agent_google_connections`), records the client that minted it in
`oauth_client_id`. It refreshes with that client wherever the client now
sits: either slot, or an env pair that an Admin record shadows. The slot
decides only which client *new* connects use.

- **Recorded on connect.** Each connect callback records the client its
  exchange ran on. If Google sends no new refresh token on a reconnect, the
  old token stays, and so does the client recorded for it.
- **Connections from before.** The upgrade records the Admin UI's
  connections client on every stored connection, because that is the client
  every refresh ran on until now. An install configured only in `ui/.env`
  has no record to read. Its connections record their client on their next
  refresh. Each configured client is tried in order (connections, the env
  pair it shadows, sign-in, then its env pair), and the first one Google
  accepts is recorded.
- **A client configured nowhere.** A connection whose client is no longer
  configured is never sent to Google. Agents, Home and the cards read *"This
  Google connection was granted to OAuth client `<id>`, which is no longer
  configured on this server. Reconnect it, or an admin can add that client
  back under Admin → Google Workspace."* Agent routes, Home and a person's
  artifact export to Drive answer it as a 409 `reconnect_needed`, the daily
  brief keeps it as its schedule line, and an approval releases back to
  `pending` with it.
- **Google refuses the client.** For `invalid_client` (a wrong secret
  pasted, a deleted client), or `unauthorized_client` from the client the
  connection names, the connection is kept and `reconnect_reason` stores
  Google's words, such as
  `invalid_client: The provided client secret is invalid.`.
  `reconnect_flagged_at` stores when that refusal was first seen. Clearing
  the connection would disconnect everyone on one bad paste, and would open
  the login domain gate, which reads the org connection's refresh token. The
  next successful refresh clears the flag, and so does saving a new secret
  for that client.
- **Google refuses the grant.** `invalid_grant` still clears the
  connection. For a connection with no recorded client, it is cleared only
  when at least one configured client answered `invalid_grant` and every
  other one answered `invalid_grant` or `unauthorized_client`. RFC 6749
  §5.2 defines `unauthorized_client` as *"The authenticated client is not
  authorized to use this authorization grant type."*: the client
  authenticated, but the grant is not its own. That is read from the
  `error` code, never the sentence. When a client refused itself
  (`invalid_client`) or Google could not be reached, the connection is kept,
  and in the first case flagged with each client's answer, by id. A flagged
  connection with no recorded client reads *"Google refused this connection
  with every OAuth client configured here: «…». The connection was kept.
  Reconnect it; if Google refuses again, an admin needs to check those
  clients under Admin → Google Workspace."*
- **A roll-back's limit.** A connection is only ever sent to the client it
  names. If the previous release stores a reconnect after a roll-back, the
  row keeps its old client id. If the slots were swapped in between, that
  connection names the wrong client, and its next refresh is refused
  (cleared on `invalid_grant`, flagged otherwise) until the person
  reconnects on this release.
- **Where it shows.** `GET /api/integrations/google`, `GET
  /api/integrations/google/org` and `GET /api/fleet/defs/{id}/google` carry
  `clientId` and `reconnectReason`. The personal, assistant, org and agent
  cards show the reason with a Reconnect. `GET /api/admin/google-client`
  carries `census`: every live connection counted per client, with the slot
  that client sits in now (`shadowed` when it is only the `ui/.env` pair
  behind that slot's Admin record, which refreshes what it minted and mints
  nothing new), how many are flagged, and the connections not yet recorded.

**Not yet captured:** Google's answer to a refresh token presented with a
client that did not mint it. The trial's outcome is the same whether that
answer is `invalid_grant` or `unauthorized_client`: a live connection moves
on to the next client, and a dead one is cleared. Only an `invalid_client`
there would differ: the dead connection would be kept and flagged, which is
the safe side. A connection that names its client is only ever sent with
that client.

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

and enable the **Google Drive API**, **Google Calendar API**, **Gmail API**,
**Google Sheets API** and **Google Slides API** — the agents' sheet and slides
tools call the last two (listed with console links under Admin → Google
Workspace · OAuth client; **Test** next to the connected org account probes
all five live — consent succeeds even with the APIs off, so the probe is how
you find out, and a disabled API shows Google's own sentence and its
activation page for your project). Add the
`drive`, `calendar`, `calendar.events`, `gmail.modify`, and `gmail.send`
scopes to the consent screen. `drive` / `calendar` / `calendar.events` are
sensitive; `gmail.*` are restricted.
For an **Internal** (Workspace-only) OAuth app none of these need a verification
review; a public/External app would (especially the Gmail scopes).

## Verified vs. trusted

**What a revoke ends: documented by Google, not exercised live.** Google's
[OAuth 2.0 for web server applications](https://developers.google.com/identity/protocols/oauth2/web-server)
page, under "Token revocation", says: "Revocation removes all OAuth 2.0 scopes
previously granted to a project, invalidating any issued access or refresh
tokens for all clients registered under that project." The shared-grant check
before a disconnect's revoke rests on that sentence. The scope it names is the
Google Cloud **project**, not one OAuth client: a second client in the same
project (a sign-in client, say) shares the grant. So the check matches stored
connections on the Google account alone, whichever client minted them. No
revoke of a real grant has been made here; every revoke exercised so far sent
a token Google never issued.

The connect redirect (offline + `WORKSPACE_SCOPES` + `prompt=consent`), the token
encryption round-trip/tamper-rejection, the DB model, and the export permission
gating are all verified live. The **actual Drive round-trip** (create Doc/Sheet)
requires real Google client credentials + a consented account and cannot be
exercised in a headless dev env — it's wired to Drive's documented multipart
convert-on-upload API but is the one path proven by the API contract rather than
a local screenshot.

## When Google refuses — whose words you read

Every refusal reaches the person or agent in **Google's own words**, quoted, not
replaced by a Talaria sentence. Each Google crate keeps Google's status and body
on its failure (`<what> failed: <status> <body>`), and
`talaria_google_errors::upstream_reason` lifts out what Google wrote. A REST
body gives its most specific code, then its message. The code is the
`ErrorInfo` reason (`SERVICE_DISABLED`), else `status` (`PERMISSION_DENIED`),
else the legacy `errors[].reason` (`notFound`, which is all Drive v3 sends). An
OAuth body gives `error`, then `error_description`, through the one parser of
that shape (`talaria_google_connections::oauth_reason`), which a disconnect's
refused revoke quotes through too. The quote is capped at 300
characters, and the code always leads, so a cut never loses it. Nothing matches
a class of refusal from wording Google has not been seen to send. That includes
the "client not marked for AI agents" refusal from Google's Workspace MCP
servers, which is not yet captured.

| Where | What the reader gets |
|---|---|
| A Google call an agent or person made (`google_fail_with`) | 502 `Google <Product> refused: «SERVICE_DISABLED: Google Sheets API has not been used in project … Enable it by visiting …»`. A scope refusal is still the 409 `reconnect_needed`, with `Google said: «…»` appended. Only a failure with no Google body (a timeout, a local fault) still reads `Could not reach Google <Product>.` |
| An agent's token (every `/api/integrations/google/agent/*` route and the agent arm of `POST /api/artifacts/{id}/export/google`) | `resolve_agent_google` no longer turns a failed token into "No Google account is connected". `invalid_grant` is a 409 `reconnect_needed`, which quotes Google and says the stored grant was cleared. `invalid_client` / `unauthorized_client` is a 502 `client_rejected`, which quotes Google, keeps the connection, and names the admin's fix. Anything else is a 502 that quotes Google when it answered. |
| Approving a held action (`POST /api/integrations/google/pending/{id}`, chips, Inbox, the brief) | A token that fails at approval **releases the row back to `pending`**. It used to sit in `executing` until the ten-minute takeover. The answer quotes Google, and the same sentence stays on the row as `result.error`. The agent's queue reads (`GET /api/integrations/google/agent/pending[/{id}]`) show it as `lastError`. A refused grant answers 409 `not_connected` (a reconnect fixes it); a refused OAuth client answers 502 `client_rejected` (the admin's fix), and any other refused refresh 502 `google_error`. An action Google rejected answers `Google rejected the action: «…»`, or `The action did not run: «…»`, folded and capped, when no Google error body came back. A chip-unlocked draft that runs at once (`draft_email`, `draft_calendar_event`) answers the agent with the same sentence, and says when the draft is back in the queue as pending. |
| A connection whose OAuth client is configured nowhere here | 409 `reconnect_needed`: `This Google connection was granted to OAuth client <id>, which is no longer configured on this server. Reconnect it, or an admin can add that client back under Admin → Google Workspace.` Nothing is sent to Google. Approving a held action releases it to `pending` with that sentence and `Nothing ran.` |
| A refused OAuth client, on the cards | The connection is kept, and `reconnectReason` reads `Google refused the OAuth client this connection refreshes with: «invalid_client: The provided client secret is invalid.». The connection was kept; an admin needs to check that client under Admin → Google Workspace.` |
| A disconnect (`DELETE /api/integrations/google`, `…/google/org`, `/api/fleet/defs/{id}/google`) | The connection is forgotten either way. When Google refuses the revoke, `revokeNote` reads `Disconnected here, but not revoked at Google: Google answered 400: invalid_token.`, the same code-then-sentence quote. |
| The connect bounces (`/settings/connections?google=`, `/admin?googleOrg=`, `/agents?googleAgent=`) | `exchange_failed` and `denied` carry `&reason=<code>`. That is Google's OAuth error code from the refused exchange, or the `?error=` Google redirected with: `invalid_client`, `invalid_grant`, `access_denied`. The flash shows `Google said: <code>.` Only a bare snake_case code ever rides the URL, and the page drops anything else. The Agents page now reads its bounce; before this, nothing did. |

**Out of reach.** Some refusals are shown on Google's own consent page and are
never sent back to Talaria: `redirect_uri_mismatch`, `org_internal`, and
`admin_policy_enforced` (a Workspace admin's app-access policy). Nothing on the
Talaria side can quote them. The person reads them on Google's page. This is
how Google is known to behave; none of the three has been captured against a
Talaria instance.

# Changelog

All notable changes to Talaria. Milestone labels refer to the historical plan, [`docs/history/PLAN-design-notes.md`](./docs/history/PLAN-design-notes.md).

## [Unreleased]

### Security

- **2026-08-26 audit remediation** (full work order: `docs/AUDIT-2026-08-26.md`, PR series
  #257–#265). The one real vulnerability: both bytes routes served uploader-declared MIME inline —
  `text/html`/`image/svg+xml` executed same-origin with the viewer's session. `serveUpload()` is
  now the single disposition decision (raster + PDF inline; everything else attachment + nosniff +
  sandbox CSP). Also: credential sealing now walks array-content turns (image turns traveled whole
  — the exact case the adversarial tier measures); the one raw fetch on a model-supplied URL goes
  through safe-fetch; federation imports validate slug/department alphabets (newline → `.env`
  injection, `../` → traversal); invite email HTML escaped; timing-safe OAuth state compare; MCP
  gateway validates non-builtin upstream URLs; `/api/join` rate-limited. Deferred hardening filed
  as issues: llm.v1 per-key spend caps, stream-based upload size rejection, compose first-boot
  secret generation, upstream-error-text sanitizer pass, Google `email_verified` enforcement.

### Breaking (external SDK consumers only)

- API renames: `/api/plan/:id/doc|draft` → `/api/plans/…`; singular
  `/api/inbox/focus/conversation` folded into `/api/inbox/focus/conversations/$id` (with `current`
  sentinel); `/api/profile` → `/api/me`. Validation-failure bodies are now zod-issue 400s across
  ~90 routes (was a mix of 400/422/custom); 401/403 split fixed on four task endpoints; 500s no
  longer echo `e.message`.

### Changed

- **One owner per duplicated helper** (the audit's recurring failure mode): Google OAuth built once
  (`server/google/oauth.ts`), JSON-RPC envelope once (`mcp-jsonrpc.ts`), `errText`/`errLine`,
  `tz.ts`, `docker-exec.ts`, `asIso`, shared zod schemas (`lib/api-schema.ts`); `localMoment`,
  board-visibility SQL, MCP protocol pin consolidated. Client mutations all through
  `fetch-json.ts` (`postJson`/`putJson`/`patchJson`/`delJson`) — and the `credentials:
  'same-origin'` stanza is now census-enforced to live only there.
- **UI primitives close their gaps**: new `Popover` shell (nine hand-rolled engines deleted onto
  it), `IconButton size="tile"`, `Checkbox bare`, `StatCard href`, `StatusDot color`, Board's view
  toggle on `Segmented`; one emoji picker (chat's hand-rolled grid deleted); ~80 icon-button,
  checkbox, dot and input sites adopted. `SaveButton` deleted (0 uses; `useSavedFlash` stays — it
  has 8 importers).
- **Teams**: `renameTeam`/`deleteTeam` landed — owner-gated route + minimal UI + the functions'
  first tests.
- **Dead code out**: `adapter/`, `stack/`, the unreachable focus-inbox cluster (11 files),
  `MentionMenu`, `EmojiShortcodeMenu`, `NotificationsPanel` (live toasts are `NotificationToasts`),
  `native-tools.ts` — every deletion grep-verified. Docs now say what's real: the plugin is
  dormant/manual, backup is `bun talaria backup`, the work order lives in `docs/AUDIT-2026-08-26.md`.
- **Storage**: production refuses the published dev S3 password at the internal-mode use-time
  doors (env.ts promised this guard; it didn't exist until now).
- check-invariants grew three tripwires: inline-serving only via `serveUpload`, same-origin-fetch
  census at zero, popover census.

### Documentation

- **Docs overhaul, part 1 of 5 — restructure + truth** (first PR of the docs pass: dev docs and
  end-user docs, succinct and table-first). Ten dated/superseded docs moved to
  `docs/history/` (PLAN, PRICING, SECRETS-PLAN, PRODUCT-GAPS/PRODUCT-PLAN, three audits,
  m0-contract, TODO) with a README stating the rule: nothing there is current, the live doc wins —
  every inbound link repointed. New `scripts/check-docs.mjs` tripwire: every markdown link in the
  tree must resolve to a real file (would have caught the `m0-contract → adapter/` link that
  pointed at a deleted directory for months); wired into `bun run check`. `apps/README.md`
  rewritten to the real Svelte 5 SDK anatomy (was React-era: tsx/useQuery/react imports);
  `ui/README.md` setup section now shows the one true path (`bun talaria setup` from the repo
  root), drops its roadmap-duplicate sections, and documents per-agent `tak_` credentials instead
  of the retired org-wide `TALARIA_AGENT_KEY`.

- **Docs overhaul, part 2 of 5 — the generated references** (`bun run docs:api`, wired into
  `bun run check` as `--check`). New `scripts/gen-docs.mjs` extracts the HTTP API reference from
  the route sources — 214 routes, 343 (path, method) rows → `docs/api/` (23 group files + index
  with the auth legend): path, method, guard class (session/admin/perm/view/agent/dual/fleet/
  bearer-key/public), parseBody field tables (inline and named zod schemas, z.union variants),
  literal statuses, SSE/audit markers, and a heuristic Returns column that prints `…` rather than
  guess. The CLI reference (`docs/CLI-REFERENCE.md`) imports the real command tree — the same
  declarations `--help` renders — plus a hand-written guide (`docs/CLI.md`). Prose notes live in
  the source as `// doc:` runs (21 routes seeded); otherwise a route's own leading comment is the
  note. Guard vocabulary is closed: an unrecognized `await X(request)` renders `unknown(X)`, never
  a false `public` (this caught `actingUser`/`taskActor`/`commentReader` during the audit). CI now
  runs `bun run check` instead of the bare invariants script, so every gate added to the chain
  actually reaches CI.

- **Docs overhaul, part 3 of 5 — the full SDK docset.** `docs/sdk/` replaces the single-file
  `docs/SDK.md` (now a redirect stub, so existing links survive): getting-started, ui-kit,
  client, server, mcp, harnesses (including the previously undocumented bridge pattern:
  `runHarness`/`resolveHarnessModel` from app code), workbench-harnesses (the two-contracts
  table), and `reference.md` — every export of both entry points, one row each. New tripwire in
  `check-docs.mjs`: SDK export coverage diffs both directions against `reference.md`, so a new
  export without a row fails `bun run check`, and so does a documented symbol that doesn't
  exist (this caught `KeyHint`, which the old doc listed for months). Surface repairs the
  coverage pass surfaced: `Popover` now actually exported from `@talaria/sdk`;
  `CheckResult`, `ToolPolicy`, `ModelChainStep`, `ResolvedHarnessModel`, `RunLedger` exported
  from `@talaria/sdk/server` (all referenced by already-exported types). The harness example
  and the bridge example in `docs/sdk/harnesses.md` are transcribed verbatim into
  `ui/src/sdk/server.test.ts` and run through the real runner — the documentation is a build
  target. `docs/APPS.md` rewritten to the Svelte 5 anatomy, linking the docset.

- **Generated API reference: 22 hidden method rows recovered.** The generator's method-span
  scanner ran over raw source, so a comment inside a handler whose text contained backticks
  and an apostrophe (`` `channels.ts`'s ``) read as a phantom string to the scanner and
  de-synced its brace depth — `POST /api/channels/{id}/messages`, `PUT/DELETE
  /api/tasks/{id}`, and 19 more rows never rendered (their body schemas attached to the
  wrong row). Comments are now stripped length-preservingly before scanning, so offsets and
  `// doc:` note positions survive; 343 → 365 method rows across 18 group files.

- **Docs overhaul, part 4 of 5 — the member guides** (`docs/user/`). Eight chapters plus an
  index, one per work surface, written for people who use Talaria rather than run it:
  getting-started (sign-in, the assistant wizard, finding your way), your-day (the daily
  brief, the assistant drawer, notification classes), comms (channels, relays, DMs, agent
  chats), boards (tickets, workflow columns, saved views, review and sign-off), plan (the
  multiplayer planning conversation and its living document), research (three depths of
  cited runs), knowledge (spaces, official content, OKF), files (places, sharing, Secrets).
  Every recipe uses the UI's own words — the labels, buttons, and empty-state sentences the
  components render — traced to source while writing; the agent chapters and admin guide
  follow in part 5.

- **Docs overhaul, part 5 of 5 — agents, admins, and the glossary** (`docs/user/`). Nine more
  chapters complete the member docset: working-with-agents (meeting agents everywhere —
  mentions, chats, board tickets, the review walk, the rails), personal-assistant (the
  wizard, tuning in Settings, the brief and drafted replies, Google connection and
  delegation), templates (the ticket/plan skeletons and the resolution chain), apps
  (lifecycle, grants, the shipped reference apps), four admin chapters (people with the full
  permission catalog; the agent roster; models; MCP + observability + apps), and a glossary
  covering every term the guides use. Cross-links landed at the seams: comms and boards now
  point into working-with-agents, and getting-started's read-next covers the whole set. The
  permission table is verified id-for-id against `server/permissions.ts` (13 permissions,
  same order, same member defaults).

### Added

- **Release channels: nightly, RC, and stable images on GHCR.** A `rc`
  branch stages release candidates, a `testing` branch feeds a nightly
  (03:17 UTC), and one release workflow publishes both plus the stable
  path: a `vX.Y.Z-rc.N` tag on `rc` builds image `X.Y.Z-rc.N` + moving
  `rc` and opens a GitHub prerelease; a `vX.Y.Z` tag builds the version +
  moving `latest`; the nightly publishes `nightly` + `nightly-YYYYMMDD`
  with no Release (365 prereleases a year is tag noise with no reader).
  The channel resolution lives in one `resolve` job that fails loud on
  malformed tags — nothing silently falls through to `latest`. Publishes
  are gated by calling ci.yml as a reusable workflow, so "what gates a
  release" and "what gates a PR" are the same list and cannot drift. Git
  tags are the version authority (the `package.json` versions stay
  decorative and unread); the workflow never commits a version bump. The
  image carries its identity as OCI labels + `TALARIA_VERSION`
  (`unknown` on local builds, which is simply true of a local build).
  Operators consume a channel with a committed override —
  `docker compose -f docker/compose.yml -f docker/compose.registry.yml
  up -d --no-build` with `TALARIA_CHANNEL` pinning the tag — and the
  default checkout-build path is byte-identical to before (the base
  compose file changed comments only, verified by diffing
  `docker compose config`). The model, the tag grammar, and the
  one-time GHCR visibility flip: `RELEASING.md`.

- **`talaria` — one TypeScript CLI for everything the bash scripts did.**
  ~1,600 lines of shell across eleven scripts are now a zero-dependency
  `cli/` package run directly under bun (`bun talaria …`), with a flat
  tree: `setup`, `dev`, `worktree`, `reset`, `box`, `deploy`,
  `backup`/`restore`. Every ported script is deleted in the same commit
  that replaces it and every doc reference rewritten — no shims, no dual
  sources of truth. The CLI is dependency-injected end to end
  (`run(ctx, args)` with exec/pipe/readLine/env/isTTY/now behind one Ctx
  interface), so all 126 of its tests run against planted process
  behavior with no mocks. Safety rails survive structurally rather than
  by careful quoting: reset's SQL runs as `psql -c` argv with
  `ON_ERROR_STOP=1` (the bash's stdin-swallowing `read` trap is
  impossible now), restore streams `gunzip | psql` (dumps never sit in a
  buffer), and secrets land only in 0600 files. Two latent bash bugs the
  port's live round-trips surfaced are fixed: worktree creation died on
  container-name conflicts with the main stack whenever the main stack
  was up (its compose up is now scoped to the two services a worktree
  actually owns), and `dev` inside a worktree silently adopted the MAIN
  stack's containers under the default compose project (it now aims at
  `talaria-wt-<name>` with ports lifted from the worktree's own ui/.env).
  `deploy` wraps exactly the commands CONTAINER.md documents and prints
  each one before running it — a test reads the doc at runtime and
  asserts the argv is character-for-character identical, so the doc and
  the CLI cannot drift; it also resolves `DOCKER_GID` from the socket and
  warns when a shell export is missing from `docker/.env` (the drift trap
  the doc warns about, detected before it bites). `setup` now also installs
  a bare `talaria` command on your PATH — a two-line sh shim in bun's bin
  dir that runs the invoking checkout's CLI from anywhere (re-running
  setup elsewhere repoints it; `TALARIA_BIN_DIR` overrides the location),
  with a warning when the target dir isn't on PATH. Legacy checkouts:
  re-run `bun talaria setup` to repoint the `git wt` alias at the CLI.
  The runtime-coupled scripts (`update-restart.mjs`, `chassis.template.yml`,
  `skills/`, the smoke/invariant checks) stay where they are — CI and the
  prod image call them by path.
- **Devboxes: a containerized dev environment per task.**
  `./scripts/devbox new <name>` builds a disposable full stack per agent
  session or experiment: a local clone of the repo on its own branch
  (`agent/<name>`), private stateful sidecars (Postgres/Redis/Qdrant/MinIO)
  seeded from the primary dev environment, a private fleet project, and the
  agent CLIs — Claude Code *and* opencode — inside the container
  (`./scripts/devbox enter <name> claude|opencode`). The toolchain image
  (`docker/devbox.Dockerfile`) is glibc on purpose (bun + the npm-packed CLIs
  are glibc-first), fixes uid 1000 to match the host user so the bind-mounted
  clone needs no chown dance, and pins the CLIs with their autoupdaters off
  (version bumps are rebuilds). Networking is the production shape applied to
  dev — three networks per box: its own (sidecars by service DNS, nothing
  published — several are unauthenticated), the primary stack's network
  (shared stateless TEI/SearXNG, reached by container name), and a per-box
  `devbox-<name>-fleet` the spawned agents attach to, dialing the app
  container-to-container (`TALARIA_AGENT_DIAL=container`). State binds at the
  same absolute path on host and container, the same rule as the production
  deploy, because the fleet renderer bakes host paths into agent binds; all
  paths are canonicalized so symlinked checkouts can't leak a spelling the
  box can't resolve. Seeding is a snapshot, not a link: Postgres dump-restore
  and an `mc mirror` of the bucket always (Qdrant is opt-in `--qdrant`, being
  derived data; Redis never, being transient), with the primary's
  `TALARIA_SECRET_KEY` carried verbatim so sealed secrets decrypt. Agent-CLI
  auth is per-box in a named home volume — interactive login, a headless
  `--claude-token`, or GLM/provider env via a 0600 `compose.override.yml`
  (`ANTHROPIC_BASE_URL`/`AUTH_TOKEN`/`MODEL`; never together with
  `CLAUDE_CODE_OAUTH_TOKEN`) — a host `~/.claude` is never shared, because
  concurrent CLIs corrupt `.claude.json`. `rm` refuses unpushed work;
  teardown removes projects, volumes, the fleet network and the directory
  with no residue. Verified end to end: two boxes plus the primary ran three
  fleets on one host, both apps answered on their 53xx ports, agents reached
  their app by service name with zero published ports, and home-volume
  logins survived stop/start. Runbook: docs/DEVBOX.md.
- **A single-container deploy: production image + instance compose.** The
  whole app now builds into one image (root `Dockerfile`, multi-stage:
  bun/node build → pruned prod deps → an alpine runtime with just bun, the
  docker CLI + compose plugin, and git) and one `docker/compose.yml` stands
  up an instance — app plus postgres, redis, qdrant, TEI embeddings, minio
  and searxng, nothing published except the app itself. Configuration is
  env-only by design: the image ships no `ui/.env`, real environment always
  wins, and the entrypoint (`docker/entrypoint.sh`) generates what's missing
  — secrets into `/var/lib/talaria/env/generated.env`, first-boot admin
  credentials into the logs — so `docker compose up -d --build` with zero
  env lands on a working, signable instance, while an orchestrator that
  supplies everything overrides all of it. State is a host bind mounted at
  the same path on both sides, because the fleet renderer bakes absolute
  host paths into agent bind mounts that the *host* daemon resolves — a
  named volume would be invisible to every agent. The fleet goes
  container→container: the app joins the shared `talaria` network (a new
  `TALARIA_AGENT_DIAL=container` makes the manifest dial agents by compose
  service name instead of host-loopback ports; `TALARIA_MCP_GW_URL` and
  `TALARIA_GATEWAY_SELF_URL` point agents at the app's service name), which
  implements the "right shape" AGENT-NETWORKING.md describes — no host
  firewall rule, and agents can reach the app but never postgres. The
  fleet preflight derives its probe target from the renderer's config
  instead of a hardcoded `host.docker.internal`, so containerized instances
  stop reporting false negatives. SearXNG's settings (secret included — it
  ignores env) are rendered by a one-shot init service running the same
  image, and the updater stands down (`TALARIA_UPDATER=off` baked; deploys
  are rebuilds, Dokploy/Portainer/plain-compose all work — build-at-deploy
  time from a checkout, registry publishing left as a commented `image:`
  line). Runbook and env contract: docs/CONTAINER.md.
- **A golden-image deploy for Proxmox.** `scripts/image/` builds an openSUSE
  MicroOS template with the system half of an install baked in — Docker +
  compose v2 (podman stays unused), Tailscale, firewalld rules for the
  agent→app path from AGENT-NETWORKING.md, Bun, the guest agent — and no
  Talaria in it at all: every instance cloned from the template installs
  *current* Talaria on its own first boot (clone → setup.sh → infra → build
  → `talaria.service`), so the image can't ship a stale app. Per-instance
  configuration rides cloud-init snippets (`qm set --cicustom` →
  `/etc/talaria.env`): a handful of vars steer the bootstrap (tailnet key,
  instance hostname, repo, ref) and everything else reaches the app process
  verbatim, winning over `ui/.env` the way the environment always has.
  `TALARIA_HOSTNAME` names the whole instance — system hostname via
  hostnamectl plus a hosts(5) entry, not just the Tailscale node name —
  because the cicustom snippet replaces the user-data that would otherwise
  carry one, and every clone would answer to the template's neutral name.
  The install is
  re-entrant — first boot retries converge instead of wedging on a
  half-installed `node_modules` (setup.sh's skip-if-exists can't heal that
  on its own), the app unit gates on Postgres readiness rather than losing
  the boot race to a cached failed migration, and a re-run never deletes the
  checkout, where uploads and fleet state live outside git. systemd owns
  restarts, so the in-app updater stands down (`TALARIA_UPDATER=off`) and
  updates are a one-liner. The SearXNG settings render moved from dev.sh
  into a shared `scripts/render-searxng.sh` so the bootstrap and the dev
  loop mount the same file. `build.sh` assumes nothing about the host it
  runs on: it prompts for the VM-disk storage (from live `pvesm` output)
  and the snippet storage (offering to enable snippets on `local` by
  *merging* content types — `pvesm set --content` replaces the list), and
  when the host has no SSH public keys it generates and names a keypair to
  inject; the same choices exist as flags for non-interactive runs, and
  `--dry-run` prints the resolved plan. Full runbook:
  docs/SELF-HOSTING.md. Verified repo-side: `bash -n` on every new script,
  the generated cloud-init snippet round-trips its four embedded files
  byte-for-byte under YAML literal-block indentation, and the build.sh
  resolution/prompt/keygen flows pass a stubbed-host test matrix
  (`pvesm`/`qm` stubs, pty-driven prompts, dry-run assertions); the image
  build itself runs on the Proxmox host per SELF-HOSTING.md (not
  exercisable from this tree).
- **Apps get their own place in the sidebar.** The rail now separates enabled
  apps from Work: Work stays Talaria's own surfaces (Inbox, Comms, Boards, and
  the rest), and each app's work surface sits under a new Apps heading of its
  own between Work and Manage — you can tell platform from app at a glance.
  An app's manage surface still slots under Manage, because Manage is the
  control plane no matter who published the view. The Apps heading only
  appears when an app is enabled; with none installed the rail reads exactly
  as it did. Verified live both ways: no apps enabled (no phantom heading)
  and the contacts reference app on (Apps holds it, its manage row lands
  under Manage, and `/x/contacts` still routes).

- **Toasts, and a tap on the shoulder when you're elsewhere.** New
  notifications (a mention, a DM, a share, an approval) now surface as
  in-app toasts on every surface: a small stack bottom-right, click to go
  there, six and a half seconds and gone. A watcher in the app shell rides
  the same liveness the Home feed uses (the SSE firehose plus the 30s poll
  as the floor), so a mention that lands while you're on Boards reaches you
  there. The first read is the baseline: reloading never replays the inbox
  as a burst of toasts, and past four stacked the oldest quietly goes. The
  browser half is deliberate: an OS notification fires ONLY when Talaria is
  not what you're looking at (another tab, another window, minimized) —
  while you're looking at it, the in-app toast already said it. Permission
  is asked from your click, never on load: Settings → Notifications gains a
  Desktop notifications row (On/Off, with the blocked case explained), and
  a local off switch stands in for the grant browsers won't let a page hand
  back. Two open tabs each see the arrival; the OS shows it once (the
  notification id doubles as the OS tag). This covers the open-but-
  background case, which the plain Notification API does from a tab;
  notifying a fully closed browser is Web Push and stays unmade. Verified
  live: a row landing while focused toasts with no OS notification, the
  toast click routes to its target, the same arrival with focus elsewhere
  fires the OS notification with the right tag (headless Chromium denies
  real notifications and reports every page focused, so that leg ran
  against browser stubs plus unit tests of the gate), and a reload toasts
  nothing. 11 new tests; full suite green (2580).

- **The app updates itself, from inside the app.** Admin → Security carries an
  Updates panel: it shows the running commit, checks the remote, and an
  Update now button that pulls the latest release, installs, builds into a
  staging directory and swaps it in, then restarts the server with no shell
  involved. Manual by default; an Update automatically toggle (off until
  someone turns it on) has the scheduled check install updates on its own
  every few hours. The restart is the careful part: the build lands in
  `ui/dist-next` and is swapped for `dist` only when complete (a live server
  never serves a half-built bundle), the old `dist` stays behind one
  generation as a manual rollback, and a detached helper waits for the port,
  boots the replacement, and waits for it to answer. The new server is the
  one that marks the update done, and an update that never lands shows as
  exactly that rather than a fake success. Safety rails: a dirty checkout on
  the server is refused rather than pulled over, dev installs never update
  (vite reloads on its own), and `TALARIA_UPDATER=off` is the kill switch for
  deployments supervised some other way. Verified live: the panel's API
  answers in dev with mode dev and refuses to apply there, a check against
  the real remote reports behind/ahead honestly, the auto-update toggle
  round-trips and persists, and 9 unit tests cover mode gating, dirty-tree
  refusal, and the running → done/failed reconciliation across a restart.
- **Bun is the repo's runner, dev through production.** A root `package.json`
  is now the hub for every way to drive the repo — `bun run dev` (full
  stack), `build`, `start` (production server on the Bun runtime), `test`,
  `typecheck`, `check` (invariants), `verify` (all of it) — and both packages
  carry `bun.lock` (migrated from their npm lockfiles; versions unchanged).
  setup/dev install and build through bun, the MCP server runs its TypeScript
  entry directly in dev (`bun src/index.ts`; tsx dropped) and production
  boots with `bun server-entry.js`. Node stays a hard floor — vite, vitest,
  svelte-check, and tsc run under it — so CI installs both, pinned to bun
  1.4.0. Verified live: `bun run verify` green from the root (2559 tests, 0
  type errors, invariants pass), a full MCP stdio handshake under bun, and a
  production boot serving real API traffic against live Postgres/Redis with a
  clean SIGTERM shutdown (the scheduler's 9 jobs drained).
- **Agents can clean up an inbox, without ever being able to empty one.** Three
  new fleet tools over the connected Google account: `list_labels` (Gmail's
  folders ARE labels — INBOX and UNREAD are system ones), `create_label`
  (find-or-create, so a retry is safe), and `organize_emails` (apply/remove
  label names on up to 100 messages by id: removing INBOX archives — mail stays
  in All Mail — removing UNREAD marks read). The HITL line is deliberate and
  follows the platform's own rule: sends and invites leave the building under
  the owner's identity and wait for approval, while filing, archiving and
  mark-read stay inside the mailbox and are reversible — so they apply
  immediately, because "clean up my inbox" behind fifty approval cards is not
  cleanup. TRASH and SPAM are refused everywhere (service layer, sandbox,
  agent-facing routes), so nothing in the toolkit can delete mail. Message
  listings now carry label names alongside each message. Organizing needs the
  `gmail.modify` scope (swapped in for `gmail.readonly`); a connection granted
  before this needs one reconnect, and the routes say so when they hit it.
  Fixtures grade the two real risks: filing into a label that was named but
  never created, and reorganizing a mailbox without reading a single message in
  it — including archiving an unread mail its owner still needs.
- **Your Google Workspace account, connected to your assistant where you look
  at your assistant.** Settings → Assistant now carries the connect card:
  Gmail · Calendar · Drive, with the safety story beside the button — reads
  are live, and every email or invite the assistant drafts waits in your
  Inbox for your approval before anything sends. The Connections tab says the
  same thing instead of "Google Drive & Docs", the connect callback lands on
  the tab that interprets its result (it used to land on Profile, where the
  outcome flash never rendered), and the Inbox assistant panel offers a
  one-link "Connect Google" in its footer — right where a "can't reach your
  mail" reply actually bites.
- **The assistant can actually read the mail it manages.** Two new fleet
  tools: `read_email` (one full message by id — headers plus the complete
  plain-text body, decoded from Gmail's nested MIME tree, capped at 20k
  characters) because the listing tool only ever returned snippets, and
  `search_drive` (find files by name in the Drive the agent acts for, with
  links — read-only). Both are modelled and simulated in the fitness toolbox,
  same as every tool in the kit, and both refuse a legacy shared-key caller
  the way the existing Google tools do.
- **Google approvals show the payload, not a paraphrase.** An agent-drafted
  email or calendar event surfaces in the Inbox as a P0 approval card whose
  recommendation says "review the exact outbound payload" — which the card
  never showed. The evidence rows are now the payload's own fields: To,
  Subject, and Body for an email; When, Where/who, and Notes for an event —
  so the human approves what will actually go out, not a summary of it.

### Changed
- **The app talks like a teammate now.** Every piece of user-facing copy was
  swept: "command your fleet" language is gone (the login line is "Sign in
  and get to work.", the brand tagline is "get things done together"), the
  word "fleet" retired from anything a person reads in favor of your agents
  and the team, em dashes came out of copy (~430 across components, routes,
  and the server strings the UI shows, replaced with plain punctuation:
  two sentences, a colon, a comma), and the stiffest sentences were
  shortened and loosened. What was deliberately kept: the em dash as a
  "no value" glyph in tables, en dashes in ranges, LLM prompt prose, and
  code comments. Verified: svelte-check 0 errors, the full suite green
  (2569 tests), and a residual scan shows no em dashes or fleet words left
  in rendered copy.
- **Docs match the platform again.** Three read-only audits diffed the doc set
  against the code; what they caught is fixed: `ui/README` no longer claims a
  React/TanStack Start stack or a violet-to-magenta Mercury accent (both
  pre-Svelte, pre-gold) and says bun where it printed npm, `CONTRIBUTING` and
  `HANDOFF` match it (typecheck is svelte-check, not `npx tsc`), `mcp/README`'s
  install and example config use bun, `MCP.md` names the rendered gateway URL
  for what it is (the app's own port, not the toolkit's standalone listener),
  `AGENT-NETWORKING` gains a dated update on what the preflight covers now
  (the app port, the `/api/mcp/gw` path agents actually dial, external DNS)
  and the chassis's pinned resolvers, `WORKBENCH` explains why the built-in
  browser depends on that DNS, and per-person timezones (brief open time,
  digest arrival) are documented for the first time. Historical entries —
  dated plans, the changelog itself, the phase log — were left as history.

### Removed
- **The golden-image (VM) deploy is retired.** `scripts/image/` — the Proxmox
  build/bootstrap/firstboot scripts and their systemd units — and
  `docs/SELF-HOSTING.md`, which documented only them, are deleted. The
  container deploy (docs/CONTAINER.md) is the self-hosting path: same app,
  same env-var contract, a compose file instead of a VM template, and it
  doesn't need a Proxmox host to provision one. References repointed
  (README self-hosting pointer, doc index, CONTAINER.md intro).

- **Fleet is gone from Home.** The Fleet tab (admin-only) and its pulse view
  are deleted — Agents shows the same running state, and Observability owns
  the deep view, so a third copy was overhead nobody normal asked for. The
  home summary no longer computes fleet health (one fewer container status
  pass on every load), and the assistant's surface map no longer knows a
  Fleet destination. Old links keep working: `/fleet` redirects to /agents,
  and `/home/fleet` falls back to the Inbox like any unknown tab. Verified
  live: `/api/home` returns no `fleet` key, the Home tab strip renders
  Boards/Comms/Plans/Research/Docs with no Fleet entry for an admin, and the
  surface tests (including the `/home/fleet` fallback pin) pass.

### Fixed
- **Agent helpdesk tickets file to a board everyone can actually see.**
  `report_problem` used to land on a personal helpdesk board only the
  reporting user could open — a helpdesk nobody staffs. Problems now file to
  an org-wide Helpdesk board under the Talaria team: org-wide boards
  materialize an editor membership for every user (at ensure-time and on
  every sign-in), so the whole org sees the board the moment something files
  to it. Verified live: a synthetic agent problem posted with a real
  per-agent key found the board org-wide with all four users holding editor
  seats, and the ticket renders on /boards under Talaria → Helpdesk.
- **The default admin can still sign in when Google owns the login screen.**
  With a Google client connected the login screen is now the clean
  one-button experience it should be — Continue with Google centered, no
  password card — and the fallback is a quiet "Admin sign-in" whisper in the
  corner that unfolds a password card (Escape closes it). Verified live in
  both themes: Google button centered, no centered password form, the corner
  disclosure opens and closes, and the admin signs in through it.
- **Daily briefs file under Agents → [agent] → Briefs, not My Files.** Brief
  transcripts used to drop into the owner's root folder where nothing else
  lives. The artifact writer now ensures the per-agent Briefs folder chain,
  and a migration (two appended statements — the second catches mirrors whose
  folder the code path had already built; the checksum ledger refused the
  in-place edit exactly as designed) moves every existing mirror into place.
  Verified live: brief mirrors sit under Agents → Gregosaurus → Briefs.
- **The dithered selector on segmented controls no longer washes out its
  label.** The quiet band's ink weight is halved (0.78 → 0.5): the selection
  still reads unmistakably against the tile, but the label reads through it.
  Verified live on /agents — the highlighted option's label is clearly
  legible over the texture.
- **A migration added while the dev server runs now applies on the next
  request.** The migration runner caches its "done" promise on globalThis so
  vite's SSR module reloads don't re-open the pool — but that also meant a
  MIGRATIONS array that grew after boot was never re-run, and every query
  touching the new column 500'd ("column does not exist") until someone
  restarted the dev server, which is exactly how `preferred_effort` got stuck.
  The runner now records the array length of the last successful run beside
  the promise, and a GROWN array re-arms the run: already-applied statements
  no-op against schema_migrations, appended ones apply, and edits to applied
  statements still trip the checksum check. Production is unaffected — the
  array never changes inside a running process there.
- **The assistant panel answers with its tools now, steered by the view.** The
  sidebar conversation used to open every detached turn with "Tools are
  disabled" — disarming the owner's personal assistant made every live-state
  question ("how many tickets are on Finance?") unanswerable except by
  invention. The reply harness now arms the persona's own governed tool loop,
  and the prompt tells it which tools to reach for FIRST: the ones that match
  the view the panel is floating over (Boards → list_boards/list_tickets/
  get_ticket/comment; Knowledge → search_knowledge and the KB reads; Comms →
  channels and messaging; …), then its other tools when those cannot answer.
  The surface tool lists are server-side, keyed by the same id the briefs use,
  so a client cannot write tool names into the prompt. Inbox queue-card
  sign-offs keep their own path: propose through the command branch, confirm
  with a click — never the detached conversation. Three inbox-reply fixtures
  were reworded to the new contract (unbacked action claims still fail;
  Inbox-card action ids still fail; live-state answers must be grounded in
  tools or hedged, never invented).
- **The assistant panel follows the conversation again.** Two scroll fixes:
  sending now jumps to your message (a reader parked up in history used to
  stay parked while the reply streamed below the fold), and opening or
  expanding the panel starts at the NEWEST turn — the transcript remounts at
  scrollTop 0 on every expand, which used to open on the oldest of what
  loaded.
- **The assistant panel's composer rail matches the platform.** Attach sits
  left, and the effort chip + send/stop tile are right-aligned (the rail had
  no spacer, so everything packed against the left edge). The footer's
  standing "No tools" readout is gone — it now says "Tools on", which since
  the tools change above is the truth.
- **The assistant panel no longer runs past the app height when docked.** In
  flow mode (≥1400px) the panel's aside turned `relative` with no height of
  its own, and a block child of the collapse pane is auto-height — so the
  flex-1 transcript stopped scrolling and grew to its full content height,
  pushing the composer below the bottom of the app. The aside now carries the
  nav rail's own methodology (`h-full` inside the `h-full` CollapsePane), so
  header, scrolling transcript, and composer always compose to exactly the
  pane's height. Overlay mode (below 1400px) was already definite
  (`absolute inset-y-0`) and is unchanged.
- **The effort picker now appears on deployments upgraded past its ship.** The
  stored per-model catalog's only production writer is the model-adder modal,
  so a catalog written before the effort extraction had no levels for anyone
  and the chip stayed hidden until an admin re-opened the modal. An empty read
  on `/api/models/efforts` now runs a one-time backfill — the serving
  endpoints' catalogs are refreshed live (once per endpoint; a catalog written
  by the current build never re-triggers, and a failed refresh retries no more
  than every five minutes) — and the route answers from the fresh store. Also:
  the agent chip is gone from the chat composer rails (Comms, Plan, Research) —
  the sidebar owns the conversation partner, and the rail's right side is now
  tier, effort, then the send/stop tile.
- **Model cost autodetect is consistent now.** The price oracle only ever
  priced an endpoint's REGISTERED models — but tier routing and aliases
  attribute usage to models nobody registered (grok via a tier mention,
  gemini "-latest" aliases), which stayed unpriced forever, and id shapes
  like "~"-prefixed aliases, ":free" variants, "-latest", and trailing
  release dates never matched OpenRouter's catalog ids. The oracle now
  prices the union of registered models and every model usage has
  actually landed on (keyed by the exact usage string so the costing
  join hits), matches through alias fallbacks (strip "~", ":variant",
  "-YYYYMMDD", "-latest", vendor-prefixed suffix match), and an unpriced
  cloud usage row nudges a refresh ahead of the 6h cadence (15min
  throttle). Live result: unpriced cloud tokens 13.5M → 0.

### Added
- **The Google OAuth client is registered in the Admin UI, not a .env file.**
  Admin → Org → Google Workspace · OAuth client: paste the client ID, secret,
  and an optional Workspace domain restriction; the secret is sealed
  (AES-256-GCM) in the database and never shown again. The panel lists the
  exact redirect URIs to authorize in Google Cloud Console — account connect,
  org connect, and login — with copy buttons, which was the step the .env
  workflow never helped with. The env vars remain as a fallback for scripted
  deployments; the Admin-UI record wins when one exists, and removing it hands
  control back to the env. Google LOGIN stays env-flag-gated
  (AUTH_GOOGLE_ENABLED) on purpose — registering a workspace client must not
  silently open a new way into the instance — but the flag now works with
  credentials from either source.
- **A platform-default reasoning effort, yours.** Settings → Preferred model
  now offers a "Default reasoning effort" pick directly beneath the model —
  but only when the selected model publishes effort levels (a model with no
  ladder shows no control; there is nothing to default). The saved level
  becomes the starting pick everywhere effort is offered — Comms agent chats
  (tiers included) and the assistant panel — wherever the model in play
  supports it; models that don't simply run at their own default and show no
  chip. An explicit pick in any conversation (including auto) stays
  authoritative for that conversation, and the default re-seeds exactly when a
  model switch retires the pick. Stored on the profile beside the preferred
  model (`users.preferred_effort`); picking auto clears it. Because the
  preference travels across models, the server stores the bare level string
  and each surface applies it only against the levels that model's metadata
  vouches for — a stale level is inert, never an error.
- **Agents carry their own default effort, set where the model is picked.**
  The agent editor's model rows — main model and every tier alias — now show
  the effort chip beside the picker when the chosen model publishes levels,
  and the saved level becomes that agent's conversation default: Comms DMs
  with the agent (or the tier) and the assistant panel start there, and the
  chat routes apply it server-side when a sender made no pick, re-validated
  against the model's live levels so a stale config is inert. Precedence is
  specific-over-general: your explicit conversation pick > the agent's
  configured default > your platform default (Settings) > the model's own.
  `/api/models/efforts` answers both halves (`efforts` + `default`), and the
  persona resolver reads the configured effort from the same cached agent
  config walk that resolves capability keys — one read, one TTL, re-pointing
  an agent's model or effort follows within a minute.
- **The Comms composer is one honest tile.** The gold send tile now becomes
  the stop square while a reply streams (ChatComposer's `onStop`) instead of
  stop appearing beside submit, the "⏎ send" key-hint chip is gone — the
  component itself deleted, its three remaining uses (comms rail, the channel
  composer, the research start bar) removed with it — and the effort picker
  was rebuilt in the agent-chip anatomy: strong-border mono chip with the
  3×12 bar meter marking where the pick sits on the model's ladder, opening
  the §7 popover with a bar meter on every row.
- **Sending while a reply streams no longer interrupts it — anywhere in the
  turn.** The server queue (`queue: true` + the chained follow-up turn) has
  always existed, but a message sent during the FIRST turn's opening window —
  after the request leaves, before the response headers carry the
  conversation id back, which the server can hold for minutes behind a
  restarting agent — took the fresh-send path and started a second stream,
  forking the thread. Those messages are now held locally and flushed the
  moment the id lands (or re-sent as a fresh turn if the first one died
  without producing one); a hold never leaks across a thread switch, and
  attachments stay live while streaming because the queue carries them. Stop
  now means stop, too: the turn freezes what was on screen instead of the
  live-resume poller re-animating the server's copy until it finished anyway.
  Verified: `npx tsc --noEmit`, `npm run typecheck` (0 errors), `npm test`
  (2476 passing), plus the queue paths walked by hand in the running app.
- **Reasoning effort reaches the primary chat surfaces.** Comms agent DMs and
  the assistant panel now offer an effort chip on the composer rail, right
  aligned just left of the send tile — but only when the model's own catalog
  metadata vouches for levels. OpenRouter-style catalogs publish
  `reasoning.supported_efforts` per model; that list is extracted into the
  stored per-model catalog metadata at the same moment an admin adds models on
  /models (the live-catalog refresh the model adder already triggers), so the
  picker lists exactly the levels the provider says the model accepts and
  nothing else. A model that publishes no levels gets no chip, and its requests
  carry no `reasoning_effort` at all. Server-side, `/api/chat` and the inbox
  focus command validate the pick against the same metadata (a persona id is
  resolved to the model actually serving it, tiers included), send it as
  `reasoning_effort` on the outbound turn, and honor the effort of a QUEUED
  message on the chained turn that covers it (stamped on the message row,
  re-validated before the chain runs). Tool-offering gateway turns keep the
  `'none'` workaround — function tools and an effort cannot share a request.
  Verified: `npx tsc --noEmit`, `npm test` (2476 passing, including new
  coverage for the catalog extraction, the pool intersection, the persona
  resolution, and the transport bodies), `npm run typecheck`, plus a live
  catalog read against OpenRouter confirming every `supported_efforts` list
  arrives on a model that also advertises the `reasoning_effort` parameter.
- **Mercury turns Talaria into a focused operator workspace.** A unified
  near-black visual system now carries the full authenticated product, while
  Inbox becomes a risk-ranked decision queue with source evidence, guarded
  actions, persistent history, and an adjustable assistant panel that follows
  the operator across work surfaces — addressing the owner's own assistant by
  the name they gave it. The spec-matched composer keeps attachments,
  agent and model selection, response modes, MCP access, skills, help, and
  dictation in one conversation flow without weakening existing approvals.
- **Platform hardening: one API dialect, a modular UI kit, a complete SDK.**
  A two-sided audit (all 162 API routes; the whole component tree) worked
  through to zero: a guard module (`requireUser`/`requireAdmin`/
  `requirePerm`/`requireView`/`parseBody`/`actorOf`) replaced ~160
  hand-rolled auth prologues and ~90 body validations; 400s now name the
  first zod issue; audit logging landed on every sensitive mutation that
  lacked it (agent secrets, endpoint key rotations, gateway keys, agent
  edits, cron lifecycle, KB creates, ACL grants); a provider key moved
  out of a GET query string; org-wide views (cost, fleet, inference)
  gained real authz. The kit grew Tabs, Checkbox/Toggle, SectionHeader,
  Segmented, DropdownMenu, SaveButton, CopyButton, Chip modes, and
  EmptyState variants — then the eight worst offender surfaces were swept
  onto them at visual parity. The SDK now exports every primitive and
  type a third-party app needs. Config writes are PUT; actions are POST.
- **Talaria is an app platform.** Apps are self-contained codebases in
  `apps/<slug>/` that compile INTO the deployment and load as native
  surfaces — work views, manage views, settings tabs — on the same
  router, design system, and session as core. Each app can ship its own
  API (`/api/apps/<slug>/*` with an authenticated user handed to every
  handler), a migrations-free per-app document store, and MCP tools for
  agents that register in the MCP registry (badged "app") under the SAME
  granular governance — per-agent assignment, tool subsets, per-person
  access, gateway-enforced, dispatched in-process. Apps are
  explicit-grant: enabling one gives members nothing until an admin
  allows its views per person. Manage → Apps has Installed + Discover:
  a marketplace catalog (community + official, configurable index) and
  install-from-any-git-URL — a shallow clone into `apps/` IS the
  install; dev picks it up live, prod flags "awaiting build". Built on
  `@talaria/sdk` (docs/SDK.md); `apps/contacts` is the reference.
- **Invites + transactional email.** The third admission door: invite an
  email address, they get a branded join link (public /join page shows
  who invited them, to which org, bound to which address), and signing in
  with Google admits them, stamping the invite accepted. 14-day expiry,
  one live invite per address, instant revoke, state chips. Email rides a
  provider seam — your own SMTP (Google Workspace works with an app
  password) or Resend — with sealed secrets, masked GETs, and a
  send-me-a-test button. Invite creation survives a broken mail config
  (the error surfaces; the invite persists).
- **Sign-up domains + the instance's own address.** Two kinds of domain,
  deliberately separate. EMAIL sign-up domains (Admin → Org): prove
  ownership via a DNS TXT record at `_talaria-verify.<domain>` and anyone
  with a matching address may self-join — verification is mandatory, so
  nobody claims gmail.com. The HOSTING domain verifies by a self-fetch
  round trip: the server requests its own identity beacon through the
  candidate domain and checks the instance id that answers — proof DNS,
  routing, and TLS land on THIS deployment. Once verified it is the
  canonical base URL for MCP OAuth callbacks and links.
- **Knowledge goes Notion-lite.** Quote-anchored COMMENT THREADS with
  in-text highlights and resolve; multiplayer presence (who's here,
  view/edit); Muse IN the page — an always-on chat bar that edits the doc,
  plus select-any-text → Comment or Muse for surgical rewrites; context
  menus across reading and editing (with full table controls and a
  visible table toolbar); image paste/upload; inline artifact embeds; a
  clean Official promote/demote lifecycle (double-confirm demote) with
  agent-doc governance; and per-doc OKF summaries (GoogleCloudPlatform
  knowledge-catalog spec) — an agent-facing frontmatter+summary the
  Librarian maintains autonomously as official docs change. Space pages
  and doc pages now share one editor grammar.
- **MCP OAuth 2.1, end to end.** Servers that answer 401 get the full
  spec treatment: protected-resource metadata → RFC 8414 authorization-
  server metadata (path-aware, so github.com/login/oauth resolves) →
  dynamic client registration → PKCE, with resource indicators. No-DCR
  providers (GitHub) get a manual OAuth-app flow: Talaria shows the
  callback URL to copy and links the provider's own app portal from its
  service_documentation. Tokens seal per subject (org or user), refresh
  silently, and force a visible reconnect on revocation. Registry changes
  that alter what a running agent carries roll the fleet blue/green —
  Hermes wires MCP at process start, so a render alone was never enough.
- **Org-wide MCP with a real marketplace.** A registry of MCP servers
  governed per agent AND per person (assignments ∩ allowances, tool
  subsets everywhere, "All tools" explicit), enforced at a gateway that
  filters tools/list and rejects tools/call — config can't be jailbroken
  past the registry. The add flow searches the official MCP registry
  (latest schema) with tier ranking so real remote servers beat wrapper
  noise, brand icons, a featured business shelf, credential-driven
  install forms from declared headers, and publisher resolution for
  names the registry lacks (well-known manifests, documented endpoints).
  Per-user servers connect each person's own account under Settings →
  Connections. The built-in Talaria toolkit became a governable registry
  row too — same subsets, same gateway, identity locked.
- **Fine-grained permissions.** A 13-permission catalog across agents,
  work surfaces, knowledge, artifacts, files, templates, and models —
  resolved three layers deep (per-user overrides → org member defaults →
  shipped defaults), admins unconditional, enforced server-side by the
  same helpers everywhere. Admin → People shows per-person chips with
  override provenance, one checklist gating work views, manage views,
  and (later) app views, and agent allow-lists beside them.
- **Comms goes Slack-lite: threads, reactions, paste-a-file, edit &
  delete.** Channel messages (channels, Relays, DMs) now spawn THREADS —
  a side panel with the root + replies and its own composer; replies stay
  out of the main flow, roots carry a "N replies · when" rollup, and an
  agent @mentioned inside a thread answers IN the thread with the
  thread's own conversation as its context (replying to a reply re-roots
  Slack-style — threads never nest). REACTIONS: a quick-react palette on
  hover, chips with counts and who-reacted tooltips, click to toggle —
  and agents can react under their own identity (one of our twists).
  FILES: paste an image or drop files straight into any composer
  (channels and agent DMs) — they upload immediately and ride as
  attachment chips, images rendering inline. Plus own-message EDIT
  (inline, enter-to-save, "(edited)" marker) and DELETE (author or
  channel owner; a root takes its thread with it, the confirm says so).
  Relay conclusions summarize thread content too. The composer itself is
  now the Slack-shaped RICH EDITOR: markdown syntax as you type (**bold**,
  `code`, ``` blocks, > quotes, lists) with a formatting toolbar in a
  controls row under the full-width input, Enter sends (inside a code
  block it newlines), Shift+Enter soft-breaks, @mention and :emoji:
  autocomplete inline, and a searchable emoji picker next to attach.
  Reaction palette dismisses on Esc/outside-click/mouse-leave.
- **Platform sub-agents (Models → Platform).** Talaria's own workers are
  now first-class, named agents — separate from the Hermes fleet, each a
  model-agnostic harness with its own skills for one internal job: Muse
  (prompt-editing everywhere), Distiller (chat → private-brain distills),
  Concluder (relay closing summaries), Catalog writer (model blurbs),
  Judge (ticket outcome review), and Briefer (view briefings; fixed to
  your personal assistant by design). Each agent's model is configured
  granularly on the new Models → Platform tab — an admin pick wins while
  it routes, otherwise the job's auto chain keeps working untouched (the
  Judge's pick shares judge_config with the Guard panel, one source of
  truth). All platform work is now metered under `platform:<agent>`
  callers, so spend per sub-agent is attributable.
- **Dynamic titles (the Titler).** A seventh platform agent names things
  as they take shape: chats and plans are retitled after their first real
  exchange — but only while the title is still the mechanical truncated
  first message, so a name a user typed is never clobbered — and research
  runs get a concise title from their question the moment they start
  (shown across the run list, header, and briefings; the raw question
  remains underneath). Naming is fire-and-forget: a rate-limited or dead
  model keeps the current title, never blocks the work. A RETROACTIVE
  hourly sweep (kicked from comms reads, mirroring the distill sweep)
  names everything that predates the Titler or whose call failed —
  batched per pass, fail-fast when the model is down — so old chats,
  plans, and research runs pick up titles too. Chats can also be renamed
  by hand from the thread context menu (owner or plan collaborator);
  a hand-picked name is never overwritten.
- **Personal agents get a private RAG brain.** Every user's personal
  collection ("My knowledge") is now the assistant's long-term memory of
  that user, created lazily and bound to the owner + their assistant only:
  chat distills land there (alongside the owner-scoped ambient copy —
  search merges dedupe), and personal research reports index there instead
  of sitting unreachable in the activity index. A personal assistant now
  retrieves as its **owner's proxy** — the owner's channels, boards, plans,
  and distilled history, exactly what the owner could retrieve and nothing
  more (org agents keep their board-policy scope). Org-wide research
  reports get an `orgWide` payload + matching scope clause, so they're
  finally retrievable at all. Also fixes agent-key RAG search 502ing on a
  uuid cast in the team-binding clause.
- **Personal-content privacy pass + multiplayer research.** Personal-agent
  output is now private to its owner everywhere: PA-created documents, KB
  docs, research reports, and generated media carry the owner's
  `owner_user_id` and default to `private` (org agents keep publishing
  org-wide). Research is scoped like plans: you see your own runs, runs
  shared with you, and org-wide (agent-initiated) runs — and it's now
  **multiplayer**: a `research_members` table, `/api/research/:id/members`
  (share by email, owner-only, with notification + automatic editor grant
  on the report artifact; collaborators can leave), and an avatar-stack
  share UI in the run header mirroring plan sharing. Briefings only
  surface research you own or were invited to.

### Fixed
- **ACL audit fixes — six leaks closed.** (1) `/api/uploads/:id` streamed
  any file by id; now gated by `canAccessUpload` — owner, admin, or
  reachable through a conversation/channel/board/artifact the viewer can
  actually read (agents resolve through their board/channel grants or
  their owner's chats). (2) `/api/history` served full snapshot bodies for
  any key; now enforces per-kind ACLs (artifact + KB perms incl. space
  inheritance and editor grants, memory/skill by agent ownership,
  templates admin-only). (3) `/api/memory/:id` GET let any user read any
  agent's memory; now admin-or-owner like PUT. (4) KB search matched raw
  row visibility, ignoring space inheritance and grants — a doc in a
  private space leaked into results; now filters through the same
  effective-permission check the read routes use. (5) Artifact link
  DELETE had no read gate (POST did). (6) Research list/get had no owner
  predicate at all — everyone saw everyone's runs.
- **Templates view (Manage → Templates).** Templates managed in one place
  instead of a modal buried in board settings: tabbed by consumer
  (Tickets · Plans, with counts), deep-linked down to the selected
  template (`/templates?tab=plan&t=…`), context menus, list + detail
  layout with rendered skeleton preview, blur-saving name, and prompt-only
  agent guidance behind tooltips. The body edits in the full workspace
  editor — rich editing, Muse prompt-drafting, and NEW version history
  (template bodies snapshot on save with author attribution, served
  through /api/history like souls and skills). The old template library
  modal is deleted; board settings links out. **Muse is harnessed for
  skeletons**: hard structural rules (## sections only, 3–6 of them,
  placeholder hints or empty stubs — never content, under 25 lines, and a
  request for a finished document still returns only the skeleton) —
  adversarially verified live: a "14-step process in full detail" ask and
  a "fully written project plan" bait both came back as ~17-line
  skeletons.
- **Context menus, platform-wide.** Right-click is real now: one primitive
  (`ui/context-menu.tsx` — cursor-positioned, keyboard-navigable, portaled,
  viewport-clamped) wired onto every row, card, and tile: kanban cards and
  board list rows (open / copy link / copy ticket ref / archive), nav board
  rows, channel/relay/DM rows (open / copy link / mark-read that reads the
  REAL cursor), agent thread rows, message bubbles (copy text), knowledge
  spaces and docs, artifacts and folders (incl. copy PUBLIC link when a
  slug exists), the agents roster (the full lifecycle cluster, confirm
  texts shared with the buttons so they can't drift), research runs, and
  every home console row. Every item calls the exact function its button
  counterpart calls — a context menu is a shortcut, never the only home of
  an action (the rule lives in UI-CONVENTIONS). Native browser menus are
  suppressed app-wide EXCEPT on editable fields, where paste and
  spellcheck are real workflows.

### Changed
- **Navigation cleanup.** The sidebar is work surfaces only: the bottom
  logo is gone, the System section is gone — Settings and Admin live under
  the user menu (Admin still hard-gated by role independent of what the
  rail renders). **Compute, Cost, Audit, and Alerts merged into one
  Observability view** (`/observability`): the root is an Overview —
  alerts strip worst-news-first, generating-now + fleet temperature,
  gateway pulse, spend today, recent audit trail, each clicking through
  to its detail tab (`?tab=compute|cost|audit|alerts`). The four old
  routes are DELETED, not redirected — every reference (nav, home fleet
  tiles, server-side notification hrefs, the activity indexer) was
  rewritten to the new structure. **Models is tabbed**: Models (provider
  registry) · Roles · Access, deep-linked, with the verbose panel
  descriptions and field explanations moved into ⓘ tooltips.

### Added
- **The Inbox is a console.** Home rebuilt as one tab per work area — Inbox ·
  Boards · Comms · Plans · Research · Docs · Fleet (admin-only) — each
  pairing the area's live state with its recent activity, badges marking
  where attention is needed, quick-action cards gone. Boards gets a queue
  SIDEBAR (triage / review / blocked) with the FULL list per queue; audit
  trails (board/channel activity) and the notifications feed are collapsed
  by default. Failed work is a first-class signal: a plan whose last turn
  errored and an errored research run both show "failed — open to
  re-run/retry →" in their tabs and deep-link straight to the spot.
- **Assistant briefings, per view.** Your personal assistant opens each
  console tab with a short read on what needs you — at most five bullets,
  each SCOPE with its own prompt (delivery-lead framing on Boards,
  "who's waiting on a reply" on Comms, "what moved" on Plans,
  "ready-and-unread reports first" on Research). Briefings cover ONLY
  unreviewed things (unread, pending, in-flight, failed) and regenerate
  only when that set actually changes — fingerprinted on ids and counts,
  never on a timer. Deliberately EPHEMERAL: the summary lives in one
  replaceable row, and chatting back (right in the panel) streams through
  the assistant with the briefing as context, persisting nothing — no
  conversation rows, nothing indexed, nothing for comms-decay to distill.
- **Deep links everywhere.** The URL is now the selection across the
  platform: `/?tab=…`, `/knowledge?space=…&doc=…`, `/artifacts?a=…`,
  `/research?r=…`, `/comms?c=…`/`?a=…&x=…`, `/plan?p=…`, `/admin?tab=…`,
  `/settings?tab=…`. The old one-shot apply-then-clear pattern (which made
  nothing copy-linkable) is gone: picks are push navigations so
  back/forward walks your trail, defaults/healing are replace. Rule
  recorded in UI-CONVENTIONS → Deep links.

### Fixed
- **Skeletons were invisible.** The entire skeleton system (and tiptap
  table borders, and the grey status dots) painted with `var(--theme-line)`
  — a variable that never existed as raw CSS. One root alias fixes every
  shimmer at once.
- **The inbox took ~5 seconds; every RAG call stalled 3.6s.** The embedding
  client's configured docker-internal hostname fails DNS slowly before
  falling back — and the fallback was never remembered, so EVERY embed
  (search, indexing, health probes) paid the stall. The resolved base is
  now sticky. On top: `computeAlerts` ran its eight probes serially (now
  one Promise.all + a 15s cache; 4.3s → 7ms) and three surfaces shelled
  out to `docker ps` in the same breath (now a 5s cache). Home: 4.8s →
  ~40ms warm; KB search: 36ms.
- **Loading-state audit, platform-wide (~70 gaps, 35 files).** Every
  fetch-backed component now holds a layout-matched skeleton while in
  flight; EmptyStates render only after a query RESOLVES empty. Parent
  queries no longer block unrelated siblings (plan stage, models panels,
  boards' serial fetch, comms message pane). Real bugs flushed out along
  the way: clicking an agent before conversations resolved silently
  started a NEW thread instead of resuming the working one; the memory
  quick-add could clobber the file if used mid-load; a fleet-wide cron
  created while the roster loaded would target nobody; false states
  ("Not connected", "API keys are not enabled", unchecked policy
  checkboxes) flashed during every load.

### Changed
- **Dynamic greeting** (time-of-day pools, stable per hour), subtitle
  removed; notifications collapsed behind an unread badge so the
  assistant briefing is the working surface.
- **UI detail pass (live walkthrough, 2026-07-28).** A guided sweep of the
  whole surface, in nine moves. **Typography, two voices**: IBM Plex Sans
  for everything a person reads or types (markdown surfaces, editors, all
  form controls, board cards/lists, home rows, both content browsers); mono
  stays the chrome/identifier voice — the rule lives in UI-CONVENTIONS.
  **Dialogs take over the screen** (`Modal takeover`): content-heavy
  dialogs fill the viewport minus a gutter, constant height, content
  scrolls inside; deep editors (skill/memory/soul/config-history) open as
  ticket-style nested slide-ins INSIDE their dialog instead of stacking
  modals, with Esc peeling one layer at a time. **Agent editing reworked**:
  the skill editor no longer opens empty (it seeded from a still-loading
  fetch), memory gains quick-add + rich display, and crons got a real
  scheduling UI — pick daily/weekdays/weekly/monthly/interval with time
  pickers, see it in plain English, cron syntax generated underneath — plus
  in-place job EDITING via the never-wired `hermes cron edit`. **Retired
  agents can be deleted** (admin, confirm-gated): def + versions + secrets
  + rendered files + created-agent volumes; produced history and imported
  legacy volumes are kept. **Admin is tabbed by concern** (Organization /
  People / Agents / Retrieval / Storage / Security) and **Settings too**
  (Profile / Assistant / Connections / API keys) — with every panel's
  explanatory paragraph moved into ⓘ `InfoTip` tooltips. **The assistant
  is fully editable in Settings**: identity, handle, personality, model
  tier chips, start/stop, and inline Schedules/Skills/Memory — the
  user-friendly cut of the admin config, all owner-scoped. **Buttons never
  wrap** (`whitespace-nowrap` in the base). **Skeleton loading
  everywhere**: `Skeleton`/`SkeletonRows`/`SkeletonCard` replace blank
  panes and "Loading" strings across home, boards, tickets, agents, comms,
  browsers, cost, inference, activity, alerts, research — and the app
  shell itself now server-renders a skeleton frame instead of a blank
  page, which was the real "no content, then BAM" (the whole app was
  gated behind the client-side session fetch). Conventions for all of it
  are recorded in docs/UI-CONVENTIONS.md.

### Added
- **@mention autocomplete in the rich editor.** The thread #60 left open:
  ticket comments and descriptions NOTIFIED on mentions but their TipTap
  composers had no autocomplete — you had to guess the token. `RichEditor`
  now takes a `mentions` prop (a TipTap Suggestion popup, modeled on the
  slash-command menu): type `@` and pick from the people the mention will
  actually notify. Picks insert plain `@token ` text — the exact grammar
  the server parses and the renderer highlights, so the markdown
  round-trip is untouched. Wired on the ticket surfaces with board
  members: the comment composer and both description editors (inline +
  slide-in). Also closed for consistency: a ticket CREATED with an
  @mention in its description now notifies board members (only the edit
  path did; template-seeded descriptions can't fire it). KB/artifact
  bodies stay unwired on purpose — mentions there don't notify yet
  (eligibility undecided, recorded in TODO).
- **Universal @mentions (#60).** Coverage holes closed so a mention behaves
  the same no matter who writes it or where. Agent-authored mentions now
  notify: an agent posting to a channel (the agent-key write path returned
  before the notify call — "@jon can you check this" from an agent reached
  nobody), a streamed agent channel reply, and an agent's plan turn all
  notify @mentioned humans exactly like human messages. Ticket
  DESCRIPTIONS notify board members on change, mirroring the comment
  contract. Plan mentions got three fixes: the composer now suggests the
  plan's actual members instead of the whole org (mentioning someone who
  couldn't read the plan silently notified nobody), the notification
  deep-links to `/plan?p=…` instead of `/artifacts`, and a plan whose doc
  doesn't exist yet falls back to plan membership as the read boundary.
  And mentions are finally VISIBLE: a shared remark pass in the Markdown
  renderer highlights @tokens on every surface at once (chat, channels,
  comments, descriptions, plan turns) — code spans untouched, emails don't
  match. Still recorded as future threads: the TipTap @-suggestion (rich
  composers), research/KB mention eligibility.

### Changed
- **WYSIWYG everywhere (#46).** The remaining prose fields that still edited
  as raw textareas now use the shared rich editor: template skeleton bodies
  (the sections agents fill — now edited the way tickets render them),
  the create-agent soul draft (parity with the post-creation editor),
  assistant personality in both the wizard and the inline settings field,
  and the plan modal's AI-drafted ticket descriptions. One long-standing
  inconsistency fixed: YOUR chat bubbles now render markdown like every
  other message surface (they were the one bubble showing raw text). The
  plan document gains the fullscreen toggle (Esc exits) that artifacts and
  KB docs already had. Deliberate leaves, now written into
  docs/UI-CONVENTIONS.md → Editors: chat/channel composers stay plain
  textareas (Enter-to-send + caret mentions beat a toolbar; porting
  mentions to a TipTap suggestion is its own thread), and machine/prompt
  text (YAML, raw HTML, AI instructions, template guidance) stays mono
  Textarea on purpose.

### Added
- **Inference: live dashboard + container controls (#48).** The Compute page
  is now a real inference dashboard: a live strip showing **generating right
  now** (per agent, from streaming reply rows with a 10-minute recency clamp
  and partial indexes to keep the scan tiny), the **gateway pulse** (upstream
  calls / errors / time-to-first-byte p50+p95 over the last 15 minutes,
  in-memory — a pulse, not a ledger), **fleet temperature**
  (up/warming/unhealthy/down), and the last hour of per-agent activity; the
  page leans into a 5s poll while anything is generating or warming. The
  fleet roster finally shows the **warm-up state**: Docker's `starting`
  health phase (the healthcheck's 60s start_period) renders as a pulsing
  amber "warming up" dot instead of masquerading as up, and `up`/`unretire`
  return immediately instead of blocking the request for up to two minutes —
  the polled roster tells the story. Two new controls per agent:
  **Restart** (quick bounce, confirm-gated since in-flight replies drop;
  owners can restart their own assistant) and **Roll** (admin — the existing
  zero-downtime blue/green replacement, previously only reachable through
  config saves, now a button: fresh container warms up while the old one
  keeps serving, then drains). Both audit-logged. Verified live: restart
  showed `starting` → healthy on the roster, a roll flipped the slot with
  the old container serving throughout, and a real streamed reply appeared
  in "Generating now" with the gateway pulse moving. (Managing inference
  BACKEND containers — Ollama/vLLM lifecycle — is the ROADMAP's separate,
  bigger thread; this ships the agent-container half plus the dashboard.)
- **Proactive agent outreach (#59).** Agents stop waiting to be asked, two
  ways. **`message_user`** — the first agent→human write path: a governed MCP
  tool that starts (or continues) a real chat conversation with a teammate
  plus an inbox notification deep-linked to it (`/comms?a=…&x=…`). Personal
  assistants can only reach their owner; every agent↔person pair is
  rate-capped per day (the declined-send reason goes back to the agent so it
  can adapt). **The check-in sweep** — opt-in (master switch off by default
  AND per-agent flags, Admin → Proactive outreach): each opted-in agent
  periodically gets an automated turn through its OWN persona gateway showing
  its stale/blocked/waiting work and its recent outreach ("don't repeat
  yourself"), and acts through its normal tools — ticket comment, channel
  post, or message_user — so everything stays attributed, board-policy-gated,
  guard-visible, and ledger-metered. `outreach_events` logs every check-in
  and DM (caps, repeat-avoidance, admin visibility). Verified live: with
  nothing stale Jax replied NOTHING_TO_SURFACE; given a 96h-blocked
  high-priority ticket he chose to DM the board owner through message_user —
  message in the conversation, notification in the inbox, reasoning in the
  log — and the dedupe context correctly suppressed a near-duplicate
  follow-up; owner-only + daily-cap + unknown-target guards all refuse with
  plain-language reasons.

### Changed
- **Input consistency sweep (#49).** Every form control now speaks the same
  keyboard language, via two tiny shared helpers (`submitOnEnter`,
  `inlineEditKeys` in `ui/control.ts`) instead of hand-rolled `e.key`
  handlers: inline title/name edits (KB space + doc, artifact, ticket) commit
  on Enter and revert on Escape — Escape is shielded from modal/document
  handlers so cancelling an edit no longer risks closing the surface;
  field-plus-button rows submit on Enter (provider API key, assistant handle
  rename, agent secrets, MCP server add, rerank key, audit retention,
  federate directory, version note); the home-dashboard email compose — the
  one dialog still hand-rolling its own backdrop and raw inputs — now sits on
  the shared Modal (Escape/backdrop close for free) with the shared field
  primitives and focuses "To" on open; and the login username, provider-create
  name, and save-image title fields focus on open. Audited ~80 text controls
  across 33 files; 16 gaps fixed, the rest already followed the conventions.

### Added
- **Hermes self-review — agents review their own work before the QA judge
  does (#78).** Three moves. First, the plumbing bug that blocked all of it:
  Hermes only discovers skills outside its home via `skills.external_dirs`
  in config.yaml, and Talaria never rendered that key — the `/opt/skills` +
  `/opt/dept-skills` mounts existed in every container but the skill registry
  never scanned them. Rendered configs now carry both roots, so every
  Talaria-managed skill is a first-class enabled Hermes skill (live: `hermes
  skills list` shows them as `local/enabled` fleet-wide). Second,
  `subagent-driven-development` (official Hermes registry, MIT,
  obra/superpowers-derived) is vendored into `scripts/skills/` and seeds to
  every install — with the bundled `requesting-code-review` it gives agents
  fresh-context implementer/reviewer subagents per task. Third, the reflex:
  the talaria-toolkit skill and every rendered soul now teach "before
  `report_outcome`, self-review against the ticket's requirements —
  `requesting-code-review` for code, `subagent-driven-development` for
  multi-task plans" — the QA judge becomes the second line, not the first.
  Bonus: shared-skill seeding upgraded from copy-if-missing to pristine
  tracking (`fleet/skills/.seeds.json`) — copies still byte-identical to what
  was seeded follow canonical updates; admin-edited copies are never
  clobbered (both paths verified live).
- **Explicit plan-template picker.** Plans could only seed their living document
  from the agent's bound plan template, implicitly — the missing half of the
  chain tickets already had. The Plan surface now shows a Template picker (in
  the header, for a new plan before its first turn) listing every plan-kind
  template; the pick rides through plan creation onto the conversation
  (`conversations.plan_template_id`) and becomes the highest link in
  `resolveTemplate` — explicit pick → agent binding → none. "Automatic" leaves
  it to the agent default. The chosen skeleton seeds the doc verbatim and
  shapes every later agent rewrite. Verified live: a plan created with an
  explicit template seeds its doc from that skeleton, automatic falls through
  to the agent binding, and the pick persists on the conversation, 7/7 checks.
- **Confab guard: PII check + coaching.** A fifth check, `pii_leak`, catches
  high-precision personal data in model output — SSNs, Luhn-validated payment
  card numbers, IBANs (emails/phones stay unpoliced: they're everyday
  workspace content) — and strict mode now redacts PII alongside secrets in
  whatever Talaria persists or hasn't yet relayed. And the guard finally
  closes its loop: an opt-in **"Coach agents from findings"** toggle turns
  repeated findings (≥2 of a check in 7 days) into templated behavioral notes
  in the agent's rendered soul — per-check counts + fixed advice only, so
  flagged CONTENT still never re-enters any model's context; delivery is at
  render time, a performance review between sessions rather than a mid-turn
  correction. Verified live: a raw model echoing a test SSN through the
  public route came back `[redacted SSN]` with a caveat and an out-of-band
  finding, and rendered souls gain/lose the coaching block as the toggle
  flips, 8/8 checks.
- **Attachments reach the model everywhere.** Two asymmetries closed: channel
  replies now hand image attachments to agents as data-URL image blocks
  (1:1 chat already did — group channels were text-only), and TEXTUAL file
  uploads (markdown, csv, json, code, ...) now contribute their contents to
  the prompt in both paths — send, queued-turn history rebuilds, and channel
  transcripts — clipped like ref chips. File bytes re-read only for the
  recent transcript tail; images capped per reply. Verified live with a real
  agent: quotes a token from an attached file in a DM and in a channel, and
  perceives an attached image identically in both paths.
- **Toolkit onboarding — agents get the playbook, not just the tools.** The
  talaria MCP was attached to every agent but nothing taught them when to
  reach for it. Now a fleet-wide `talaria-toolkit` skill (seeded from
  `scripts/skills/` on render, admin-editable after, mounted read-only at
  `/opt/skills` — a mount that was documented but never actually wired) walks
  the reflexes: search before planning, keep the ticket alive, durable output
  goes in Talaria, drafts await approval, report_problem on breakage. The
  rendered SOUL header points at it.
- **`fetch_attachment` toolkit tool.** Agents can now READ the files attached
  to tickets and chats: text formats come back inline (clipped at 50k chars),
  images arrive as real MCP image blocks the model can see, and other binary
  formats report honest metadata instead of pretending. `get_ticket` now
  advertises the attachments array. Verified live: fleet render seeds skill +
  mount, MCP serves the tool, text/image/binary/404 behaviors all correct,
  11/11 checks.
- **Object storage — built-in bucket, bring-your-own, or both.** Upload blobs
  can now live in a real S3-compatible bucket instead of local disk, three
  ways: the **built-in bucket** — a bundled MinIO container (dev-compose
  `minio` service, creds via `TALARIA_S3_*` env, bucket auto-created) so you
  get durable object storage with no cloud account; any **external**
  S3-compatible service (AWS S3, Backblaze B2, Cloudflare R2, MinIO) via
  endpoint/bucket/keys in Admin → Storage (secret sealed by secretbox); and an
  optional **replica** that mirrors every new upload to a second provider as
  it lands (fire-and-forget — a replica outage never blocks an upload), with a
  "Sync all" that backfills everything already stored and automatic read
  fallback to the mirror when the primary can't serve a blob. The client is a
  hand-rolled SigV4 signer over fetch — no SDK. Each upload's row records
  where ITS bytes live (`s3+internal://` / `s3://` / filesystem path), so
  switching modes never strands a file. Connection tests do a real
  write/read/delete round-trip; a background migration moves local files into
  the active bucket. Verified live 22/22 across two runs: external-bucket flow
  against a throwaway MinIO, then built-in mode with auto-created bucket,
  replica mirror-on-upload, full sync, and replica fallback after deleting the
  primary object.
- **Ticket attachments.** Tickets now carry the same attachment chips as chat
  messages: uploaded files plus knowledge-doc/artifact refs (ACL-checked
  against the attacher, content clipped into the chip for models). Attach and
  remove from the ticket detail; changes log to the ticket's activity. Agents
  see attachment metadata in `GET /api/tasks/:id` and can now pull the bytes
  from `/api/uploads/:id` with the fleet key; agent callers can attach uploads
  but not refs (no session to ACL-check). Verified live end to end, 11/11
  checks.
- **Hybrid retrieval — keyword and meaning, fused.** Every brain now indexes
  each chunk twice: the dense embedding it always had, plus a sparse
  bag-of-terms vector (Qdrant IDF-modified, so exact identifiers like env
  vars, ticket numbers, model names, and error strings survive whole).
  Searches fuse both branches with reciprocal-rank fusion, so
  `TALARIA_EMBED_MODEL` finds the doc that names it AND "how do embeddings
  get configured" finds it too. Legacy dense-only brains keep working
  untouched until rebuilt.
- **Guided reindex — the repair path for a changed embedding model.** Swapping
  `TALARIA_EMBED_MODEL` changes vector dimensions and silently breaks every
  index/search against the old collections. Talaria now probes what the
  embedding service is actually serving (model + dimension, shown in Admin →
  Retrieval) against the LIVE Qdrant collection shape — never the registry,
  which had already gone stale once — and raises a critical alert plus an
  admin banner when they diverge (or when a brain predates hybrid search).
  One "Rebuild index" button recreates each brain in the current model's
  shape and refills it from the workspace's own records; index-don't-copy
  makes the rebuild lossless. Verified live: legacy 384d dense brains
  rebuilt to hybrid, exact-identifier and paraphrase queries both rank the
  seeded doc first, and stale points from deleted sources and
  pre-officialization grounding rules washed out in the process.
- **Confab guard: annotate and strict modes now act.** They were configurable
  but every path discarded the result — observe was effectively the only mode.
  Annotate pins findings to the flagged reply (`messages.guard` /
  `channel_messages.guard`) and renders a warning caveat under it in chat and
  channels (channels update live via republish); the public LLM route appends
  the caveat to non-streaming responses and injects one final SSE delta before
  `[DONE]` on streams. Strict additionally redacts detected secrets (keys,
  tokens, whole private-key blocks) from whatever Talaria persists or hasn't
  yet relayed, so saved copies and future transcripts stay clean. Agent-loop
  keys (`gateway_unmetered_keys`) never receive caveats — a finding must never
  re-enter a model's context; internal utility completions (judge, muse,
  research) likewise stay observe-only so parsed outputs can't be corrupted.
  Admin copy now describes what each mode actually does.

## Phase 7 — self-contained under Talaria (2026-07-09)

Everything routes through Talaria, on one network, with no Dockerfiles and
secrets encrypted at rest.

### Changed
- **Fleet routes through Talaria's gateway.** Every model spec in each rendered
  agent config is rewritten to point at Talaria's gateway (`/api/llm/v1`);
  Talaria routes each model to the provider you register on `/models`. Agents
  have exactly one upstream. Legacy litellm model names are bridged to real
  provider ids (`glm → z-ai/glm-5.2`).
- **One `talaria` docker network** for every Talaria container (dropped the
  legacy `ai_default`). The self-hosted inference server is just a registered
  provider, reached like any other.
- **Bridge eliminated.** The app reaches each agent's persona gateway directly
  on a stable published loopback port (`fleet/fleet.json` = model → url + key);
  `proxyChat`/`listAgents` read the manifest. Removed `bridge/`, `ui/Dockerfile`,
  and the legacy build-based `stack/` — **no Dockerfiles** remain; the run path
  is compose-only (official/published images) + host-run app.

### Fixed
- **Knowledge search finds space overviews.** Top-level spaces are documents
  too — search now sweeps their name + overview body alongside docs, and a
  space hit opens the space itself.
- **Research uses the same agent selector as every other surface** — the
  standard picker at the top of the rail (like Plan), not a bespoke composer
  pill. The composer keeps only the depth pill.
### Fixed
- **Plan mode plans; it no longer files tickets.** Plan-surface turns went
  to the agent with its full toolkit and no hint it was in a planning
  session, so an eager agent would create real tickets mid-conversation.
  Every plan turn (live and server-chained) now carries a plan-mode
  harness: think and decide with the teammate, read anything, create
  NOTHING; tickets come from the Draft tickets control once the plan is
  settled. Verified live with an explicit "create the tickets" bait: zero
  mutating tool calls, and the agent pointed to Draft tickets instead.
- **The plan document now actually builds as you talk.** The side-by-side
  doc only ever updated when someone clicked "Sync from chat", despite the
  empty pane promising otherwise, so new plans looked broken: you talked,
  the doc stayed blank. Every landed agent turn now triggers the sync
  automatically (unsaved manual edits are flushed first so a rewrite starts
  from them), with the rewriting overlay as feedback; the manual button
  stays for on-demand refreshes. Verified live: a fresh plan filled its
  document from the first exchange with no clicks.

### Changed
- **Agents converse like colleagues.** Every rendered soul now carries a
  voice contract: acknowledge in a sentence (or ask ONE clarifying question
  when the ask is ambiguous) before diving into tools, do the full work but
  keep the process out of the chat, and report outcomes like a busy human:
  what happened, where it lives, judgment calls worth flagging. Em dashes:
  most replies need zero. Verified live: the same agent whose replies were
  walls of process narration answered in five sentences with a sensible
  scoping question. Souls hot-reload, so this took effect fleet-wide with no
  restart.
- **Em dashes swept from the platform's own copy.** Around 120 gratuitous em
  dashes across 35 files of visible UI copy (placeholders, tooltips, empty
  states, hints) rewritten with ordinary punctuation. The dash survives only
  where it means something: empty-value glyphs in tables and the brand
  tagline. Codified in docs/UI-CONVENTIONS.md.

### Fixed
- **Queued chat messages get their reply on screen.** Sending while an agent
  was still replying queued correctly server-side, and the follow-up turn was
  generated — but the chat never showed it (it only appeared after a reload).
  The chat now keeps watching whenever the last visible message is the
  user's, so the server-chained reply streams in on its own. Verified live
  with a queued mid-stream message.
- **Agents can edit the docs they authored.** Editing a KB doc the agent
  itself created returned 403 (edits required an explicit editor grant, and
  create_kb_doc granted nothing) — so agents worked around it by creating
  duplicates. Authorship now grants edit; everyone else's docs still need a
  grant.

### Added
- **Agents can open a knowledge space — create_kb_space.** "Put this in a
  new Company space" no longer needs a human errand first: agents can create
  a space (find-or-create by name, so retries never duplicate), while
  sharing, deletion, and marking docs official stay human calls.
- **Agents reach for Talaria first — flailing fixed at three layers.** A real
  transcript showed an agent burning 20 tool calls hunting for Notion/
  Obsidian/vaults when the answer was one toolkit call away. Now: (1) every
  rendered soul carries a **toolkit contract** — the talaria MCP is the first
  reach for anything workspace-shaped, with the tool names spelled out and a
  hard "there is no Notion/Obsidian/Airtable" line; (2) the Hermes image's
  **conflicting bundled skill packs** (note-taking/obsidian, productivity's
  notion + airtable + google-workspace, the ungoverned email pack) are pruned
  on every roll and fresh boot — surgically, everything else stays; (3) the
  gaps that CAUSED improvisation are closed: **create_kb_doc** (the "add to
  knowledge base" job was literally impossible), **list_teammates** (resolve
  a name to an email for drafts/board shares), **list_board_members**,
  **list_research**, and a folder param on create_document. Verified live:
  the same task that flailed now completes in two calls.
- **Agents fail gracefully — report_problem.** When something breaks on the
  agent's side, it no longer dumps endpoints and error internals on
  non-technical teammates: the new tool alerts every admin, files a
  **Helpdesk** ticket with the technical details (board find-or-created),
  and hands the agent plain-language reassurance to relay. The soul contract
  teaches the etiquette; a new critical alert fires when the fleet's MCP
  endpoint itself is unreachable (the root cause behind "connection refused"
  flailing).
- **Attach anything — knowledge, artifacts, or files.** The composer paperclip
  is now a menu: attach a knowledge doc or an artifact (search pickers) or
  upload a file. Knowledge/artifact picks become reference chips on the
  message — the referenced content travels to the model on that turn and on
  every later history rebuild (queued turns, resumes, channel transcripts),
  ACL-checked against the attacher, with truly-private items silently
  undiscoverable. Chips render in history and link back to their source.
### Changed
- **Chats are keyboard-first — the send button is gone.** Enter sends
  everywhere (Esc stops a streaming reply); the enlarged "⏎ send" / "esc
  stop" key chips beside the input are the affordance, fading in when live.
  Composer controls all sit on one optical line, with depth/agent/tier pills
  hugging the input's right edge.
- **UI consistency pass — one app, not fifteen.** A full audit (screenshots +
  code inventory, docs/UI-CONVENTIONS.md is the contract) and the first big
  unification: shared surface primitives (`RailSurface`/`Rail`/`Stage`/
  `StageHeader`/`RailRow`/`CountPill`, `IconButton`, `Chip`/`StatusDot`/
  `DangerLink`). Plan's sidebar moved to the LEFT like every other surface and
  gained a real header; Research, Comms, Knowledge, and Artifacts all sit on
  the same w-72 rail with the same h-12 header line running straight across.
  Fewer, smaller controls: contextual actions are icon buttons with tooltips
  (no more giant "+ New space"/"+ New" primaries), Send/Go became one icon
  affordance (Enter submits everywhere), the reranker panel autosaves on
  change (only the API key keeps an explicit save), destructive actions are
  quiet red links instead of buttons, and ellipsis is gone from all UI copy.
### Added
- **Brain routing everywhere content lives.** The same "Brain" control now
  sits in the top menu of BOTH knowledge docs and artifacts (docs, sheets,
  microsites): Auto / a specific brain / None. For artifacts, explicit
  assignment indexes the rendered content (sheets become tables) into that
  brain only — plan documents and research reports routed away from Auto
  leave the activity brain, and flipping back restores them. Content edits
  re-index in whatever home the routing says. Owner-only on both surfaces;
  privacy still trumps routing.
- **Per-doc brain routing — brains contain only what's assigned to them.**
  Every KB doc gains a "Brain" control (owner-only, next to Official): **Auto**
  (space binding / official→org rules), a **specific custom brain**, or
  **None** (never indexed). Explicit assignment always wins, re-placement is
  immediate, and privacy still trumps routing — a private doc only ever
  reaches its owner's personal brain. Members can see brain names for the
  picker; the binding matrix stays admin-only. **OpenRouter** joined the
  reranker registry (US) — and it reuses the LLM endpoint key you already
  registered, so reranking needs zero extra setup.
- **The RAG stack lives — and got a real retrieval pipeline.** The retrieval
  plane (Qdrant + a native CPU embedding model via TEI, default
  `bge-small-en-v1.5`) is now part of the self-contained compose — it had been
  silently dead since the Phase-7 stack cut, with every index call swallowed.
  Three defenses so that can't recur: a critical **alert** when either service
  is unreachable, a one-click **"Reindex everything" backfill** (Admin →
  Retrieval; content-hash idempotent) that restored the workspace's history,
  and a **15-minute incremental sweep** that self-heals missed rows after an
  outage. Retrieval gained a **reranker** precision stage: a provider registry
  like LLM endpoints — self-hosted TEI, Voyage AI (US), Together AI (US),
  NVIDIA (US), Pinecone (US), Cohere (Canada), Jina (Germany) — with live
  model catalogs where providers expose one, sealed API keys, and graceful
  fallback to vector order (reranking can never break search). And RAG brains
  are finally **curatable in the UI**: create a brain, bind who can search it
  (teams now supported alongside users/agents/everyone), and point KB spaces
  at it — every non-private doc in a bound space feeds that brain instead of
  the org default, re-routing immediately on bind/unbind.
- **Artifacts file themselves — every agent gets a cabinet.** Auto-created
  artifacts stop piling up at the root: each agent gets a folder named after
  it, with category subfolders created on demand — **Plans** (plan documents),
  **Research** (reports), **Documents** (agent-authored docs via MCP),
  **Media** (image saves without an explicit folder), and **Chat summaries**.
  Distilled idle chats now ALSO become browsable artifacts (private to the
  chat's owner) instead of living only in the activity brain. Filing is
  best-effort by construction — a folder hiccup can never kill the flow
  creating the artifact. Humans' hand-created artifacts and explicit folder
  picks are untouched.
- **Research view (#56) — Perplexity-grade cited research, run by YOUR agents.**
  Ask a question on `/research`, pick a depth — **Recon** (one fast pass, a
  cited answer), **Brief** (planned angles, a briefing document), **Expedition**
  (iterative deep dive with gap-chasing rounds, a full report) — and whose
  expertise should drive it. The pipeline runs server-side, outside any chat
  context: the chosen agent's own persona plans the queries, judges the gaps,
  and writes the document, while Perplexity sonar models (through the org
  gateway, metered) run the search stages and supply sources. Every factual
  claim carries an inline [n] citation against a deduped global source
  registry; unresolvable markers are stripped and a mechanical Sources section
  is appended. Reports are org-visible doc artifacts (versioned, shareable,
  exportable), indexed into the activity brain, with a completion notification
  deep-linking back (`/research?r=`). Every agent gets `research` +
  `research_status` MCP tools — an agent researches its own field without its
  conversation window ever swallowing a search dump.
- **Model Roles — tailor the model stack per activity.** `/models` gains a
  "Model roles" panel: assign which model handles each class of work — a
  **search model per research tier** (Recon / Brief / Expedition — Perplexity's
  sonar family maps one-to-one; pointing a tier at a deep-research-class model
  makes the engine run fewer, bigger stages instead of multiplying effort) and
  **Utility** (catalog blurbs, chat distills, Muse fallback) are live;
  **Image understanding**, **Image generation**, **Embeddings**, and
  **Reranker** slots are reserved for their surfaces. Unset = auto (sensible
  pick from what's registered); an assignment only wins while it still routes,
  so a deleted model can never silently break a subsystem. Admin-only, audited.
- **Multiplayer Plan — several humans, one plan.** Plans are no longer
  owner-private: the owner shares a plan by email (avatars + share control in
  the plan header), and collaborators get the whole surface — the conversation
  (they talk to the same agent; turns carry author names so voices stay
  distinct, in the UI and in the agent's transcript), the living document
  (auto editor grant, revoked on leave), ticket drafting, and agent Sync. A
  "Shared with you" sidebar section surfaces plans riding other agents;
  sharing notifies with a `/plan?p=` deep link; presence rings show who's
  viewing right now. Owner shares/removes; collaborators can leave. The doc
  stays owned by the plan's owner, and @mention notifications now reach every
  collaborator (they're doc readers by construction).
- **QA judge scores against the ticket template.** At quality review the judge
  now resolves the same template chain ticket creation uses (assignee binding →
  board default) and receives the skeleton as an objective rubric: every
  section must be meaningfully addressed ("n/a" only where truly inapplicable);
  missing or skeleton sections come back as named "revise" issues.
- **Brain-routability health — unroutable agents surface instead of freezing.**
  Provider pools churn under no-train routing; when an agent's configured
  model drops off its endpoint, chats used to hang silently. Every enabled
  agent's config targets (main / tiers / fallbacks) are now probed against the
  gateway registry (30s cache): an unroutable MAIN raises a critical alert
  ("X's brain is unroutable") and a red chip on the agent's card; dead
  tiers/fallbacks get a warning + amber chip. Fix from /models or the agent.
- **Comms follow-through: unread badges, DM notifications, thread peek.**
  Per-member read cursors (`channel_members.last_read_seq`) drive unread
  count pills on every channel/relay/DM row; having a channel open marks it
  read live. A DM message now drops an inbox notification outright (deduped:
  while one sits unread, further messages fold into it), deep-linking back to
  the conversation via `/comms?c=<id>`. Agent rows in the sidebar gained an
  expand chevron so you can peek at an agent's threads without selecting it.
- **Elevated assistants — promote an admin's assistant to org-wide view/edit.**
  Admin → Users gains an "elevated assistant" toggle on admin rows
  (`agent_defs.elevated`). An elevated personal assistant reaches every live
  board (tickets + governance as editor), every channel and relay, and gets
  implicit editor rights on org-visible knowledge docs and artifacts. Hard
  lines that elevation never crosses: human↔human DMs, other users' private
  items, owner-only actions (board team moves, deletes, sharing changes).
  Elevation is only effective while the owner is an admin — demote the human
  and the assistant's reach collapses with them (demotion also clears the
  flag). Audited (`user.assistant_elevated`).
- **Drag boards between teams.** In the nav rail, a board's owner can drag it
  onto another team group (or Personal) to move it — groups highlight as drop
  targets, empty groups say "drop here". Server-side the move is owner-only
  (it changes who can see the board): `PATCH /api/boards/:id { teamId }`, or
  `{ teamName }` by name ("personal" clears the team).
- **Personal assistants can join group channels — behind a privacy gate.** The
  hard block is gone: your assistant can be added to a shared channel (still
  only by YOU — someone else's assistant never shows up in your picker). Its
  group replies carry a privacy gate above channel instructions: never reveal
  the owner's private context (memory, mail, calendar, private docs) outside a
  DM with the owner, and never use owner-identity tools on a channel's behalf —
  it declines and points people at the owner.
- **Ask your assistant to run your boards.** A personal assistant now acts as
  its owner for board governance (`actingUser` identity proxy): move a board
  between teams, share/unshare it by email, and allow/remove agents. Five new
  MCP tools — `list_teams`, `move_board_to_team`, `add_board_member`,
  `remove_board_member`, `set_board_agents` — assistant-only by construction
  (general agents get 401; the routes resolve identity server-side, and team
  moves still require the owner role). `list_boards` now shows a personal
  assistant its owner's boards (with the owner's role) alongside its
  policy-allowed boards, and `GET /api/teams` answers to the identity proxy.

### Changed
- **Inbox is tailored to you AND the org.** Two zones: the personal column
  (notifications, approvals, your triage/review/blocked queues, agenda, mail,
  quick cards, assistant) beside an org rail titled with the business name —
  fleet health, a live activity **Pulse** across boards/comms/fleet, and for
  admins two glance tiles: live alert count and today's spend (both deep-link).
  Members see the pulse without the admin numbers.
- **Home and Inbox merged.** `/` is now **Inbox** — the top nav item and the
  landing surface: notifications up top (mark-read on open, mark-all-read, the
  panel disappears when quiet), then the day's dashboard (assistant, approvals,
  agenda, mail, triage/review/blocked queues, fleet glance). The unread badge
  moved to the top-level item; `/inbox` redirects; quick cards point at the
  current surfaces (Comms · Plan · Boards · Artifacts).

### Added
- **The Talaria toolkit is ATTACHED — every agent has its tools.** talaria-mcp
  grew a fleet HTTP mode (stateless streamable-HTTP, per-request identity via
  X-Agent-Name, fleet-key auth); the app self-hosts it as a supervised child
  (probe-guarded, respawning) and every rendered config now carries the
  `talaria` MCP entry automatically. Agents get the whole safe surface —
  tickets, artifacts, channels, KB, `save_image_artifact` — on their next
  roll. Closes the long-standing "HTTP transport for containerized agents"
  backlog item (#58).
- **Agents speak product, not plumbing.** The org soul header now instructs
  agents to point teammates at workspace surfaces (Artifacts, boards, docs)
  instead of file paths and containers, unless the person is working at that
  technical level.
- **Save agent images to Artifacts.** Every agent-produced image in chat gets
  a hover "Save to Artifacts" (title + folder picker) that copies it out of
  the agent's container into a durable file artifact. Agents can do it
  themselves too: the talaria MCP grew `save_image_artifact` (path + title +
  folder-by-name, find-or-create) — agent saves default org-visible so the
  team actually sees them. For science. And company meme folders.
- **Agents can show images in chat.** Files an agent creates in its own
  container and references as `MEDIA:<path>` render inline in DMs and
  channels, streamed through `/api/agent-media/:model` — gated on the same
  access as chatting with the agent, restricted to images under the agent's
  own `/opt/data` volume (traversal-proof, size-capped, nosniff), slot-aware.
  The rewrite happens at render time, so past messages light up too; remote
  image URLs in replies already rendered via markdown.
- **Send while the agent is replying.** Agent chats flow like Claude: messages
  sent mid-reply queue into history without interrupting, and when the current
  reply finishes the server automatically runs the next turn covering
  everything queued (chaining until the conversation goes quiet, surviving
  reloads — the follow-up turn is server-driven). The composer stays live
  during streaming (Stop and Send side by side); dead streams can't wedge a
  conversation (10-minute staleness guard). Applies to agent DMs and Plan
  chats; channels were already non-blocking.
- **Org-voice model blurbs.** Model descriptions get ONE rewrite pass into
  task-oriented one-liners ("what it's good at, when to pick it") in the org's
  voice, cached in `model_blurbs`; newly registered models get theirs on the
  next catalog read (throttled, detached). Raw public-catalog text is the
  fallback; nothing is invented for unknown models.
- **Learned parameter support at the gateway.** When an upstream 400 names a
  parameter we sent (newer models retire tunables — sonnet-5 rejects
  `temperature`), the gateway strips it, retries, and remembers per
  endpoint+model so later calls pre-strip. Dynamic specs straight from the
  provider — no tables to maintain.
- **Member model access + human-friendly model picking.** Admins choose which
  models non-admins may pick for AI drafting / preferred model (Models →
  Member access; empty = all, admins never restricted), enforced server-side
  in the catalog, the preference save, and muse resolution (a restricted
  preference falls back). The picker itself grew up: models show a pretty name
  and a one-line "what it's good at" blurb, populated automatically from the
  public catalog (no maintained lists; unknown/self-hosted models simply show
  their id).
- **Rolling agent replacement — edits never kill a conversation.** Each managed
  agent runs in one of two compose slots; applying a change brings the incoming
  slot up on a **fresh port** beside the old container, cuts the manifest over
  only after real health (the app re-reads it per call, so traffic shifts
  instantly), drains in-flight replies (`TALARIA_ROLL_DRAIN_SECONDS`, default
  45), then retires the old container. A newcomer that never gets healthy is
  discarded — the old agent never blinks. Org saves and config/MCP applies roll
  instead of restarting; `proxyChat` additionally holds-and-retries through any
  residual gap instead of failing (or answering with the mock).
- **Organization config — agents join YOUR team.** Admin → Organization sets
  the business name + what it does. Woven in automatically everywhere agent
  identity forms: muse-generated agents/souls/personalities anchor to the
  business, and every rendered SOUL.md opens with an org header (a render-time
  projection — stored souls stay clean, existing agents pick it up on the next
  render/restart). Agents stop introducing themselves as "on the Hermes team."
- **Comms — every conversation in one place.** Chat and Channels merge into a
  single Slack-shaped, agent-native surface (`/comms`): persistent **#channels**
  (ambient talk), **Relays** (named ad-hoc gatherings of people + agents around
  a purpose), **teammate DMs** (human↔human, riding the channel machinery,
  deduped per pair), and **agent DMs** (durable 1:1 threads). One sidebar, four
  sections; old `/chat` and `/channels` routes redirect.
- **Conversations decay instead of accumulating.** Relays **conclude**: a
  summary of what was decided is posted as the final message, indexed for
  retrieval (channel-membership ACL), and the relay archives. Idle agent DMs
  (default 14 days, `TALARIA_CHAT_TTL_DAYS`) are **distilled** — durable
  substance summarized into the activity brain, owner-scoped — then archived
  out of the sidebar. Sweeps run opportunistically (throttled hourly, never
  blocking a request); plans are exempt (they're documents, not scrollback).
- **Ticket & plan templates.** An org-wide template library (markdown skeleton
  + agent guidance per template — the headings are the schema): boards bind the
  ticket templates they use and mark a default (Board settings → General);
  agents can carry overrides (agent modal → Summary → Templates). Resolution
  everywhere: explicit pick → agent binding → board default → freeform. Applied
  when agents draft tickets from plans/channels, when the plan document is
  created/synced, and at ticket creation itself — a bare ticket (quick-add or
  an agent's create tool) is seeded with the resolved skeleton.
- **Dependency-aware ticket drafting.** Planners propose `dependsOn` ordering
  between drafted tickets; the review modal shows/edits them as "blocked by"
  chips, and creation wires real ticket dependencies. The review modal itself
  is board-first (the board's template shapes drafts), roomier (wide layout,
  full description editing), and numbered for dependency reference.
- **Generation-in-progress states.** A shared `Generating` treatment (shimmer
  skeleton lines + stepped dots, plus an in-place overlay variant) replaces
  button-label-only waits: drafting tickets shows skeleton proposal cards,
  the plan document veils while the agent rewrites it, cron drafting shows a
  designing row, and ticket creation counts down.
- **Plan view, phase 2 — the document lives.** "Sync from chat" has the plan's
  own agent rewrite the living plan document from the conversation so far
  (`POST /api/plan/:id/doc`, metered like any chat turn; agent preamble and
  code fences are stripped). Draft tickets now treats the plan document as the
  curated source of truth, with the transcript as supporting context.
- **@mentions on the plan surface (#60).** The plan composer autocompletes
  teammates (shared mention machinery extracted from channels into
  `components/chat/mentions.tsx`); mentioned users are notified once they can
  read the plan's document (owner-private plans mention silently until shared).
- **Plans feed the activity brain (#63).** Plan turns and the living plan
  document are indexed into the ambient activity collection, ACL-scoped to the
  plan's owner (`planOwnerId`) — private planning never surfaces for anyone
  else. Hand edits to the doc re-index via the artifact save path.

### Fixed
- **Fresh-install model selection.** Bare model ids that contain `/`
  (OpenRouter-style, e.g. `qwen/qwen3-14b`) were mistaken for
  `endpoint/model` pins, leaving the preferred-model picker empty and the muse
  with "no models configured". The gateway catalog now tags qualified ids
  explicitly (`GatewayModel.qualified`).
- **No-train routing pool is fetched live.** The OpenRouter US no-train
  provider pool comes from `GET /providers` (US datacenters/HQ) on every call
  (briefly cached) instead of a hardcoded six-provider list that had gone stale
  and 404'd models it no longer served ("No allowed providers are available").
  A stored `only` list is only the offline fallback.
- **Provider catalogs are always live.** Preset seed model lists are gone —
  adding a provider drops straight into the endpoint's manage modal, where
  models come from the provider's live `/models` catalog: full list browseable
  on focus (the old picker capped at 8 alphabetical matches, hiding newer
  models), provider ordering preserved (OpenRouter lists newest first), and
  pagination followed (Anthropic pages at 20 by default).
- **Fleet network self-creates.** `fleetUp` ensures the external `talaria`
  docker network exists before `compose up` — a fresh install no longer fails
  with "network talaria declared as external, but could not be found"
  (`setup.sh` also created the wrong name, `talaria-fleet`).

### Security
- **Provider API keys encrypted in the DB**, not in configs. Sealed with
  AES-256-GCM in `llm_endpoints.api_key_cipher`; entered on `/models`, never
  returned to a client. Existing config keys are migrated into the DB
  automatically.
- **Envelope encryption + one-click rotation.** A random 256-bit DEK encrypts
  every secret and is stored wrapped by the root secret, so the
  unlock-everything key is never in a config. Admin → Encryption rotates the key
  and re-encrypts every secret (provider keys, agent secrets, OAuth tokens) in a
  single pass. All symmetric AES-256 — post-quantum-safe (no asymmetric crypto).

## [Unreleased]: Phase 6 — product depth (2026-07-06)

Turning the elegant shell into a capable product: one place to manage each
agent, attachments, personal assistants, governance, and a real audit trail.

### Added
- **Unified agent management modal**, one modal per agent with tabs — Summary ·
  Config · Skills · Memory · MCP · Versions. Every internal (previously separate
  top-level pages) lives here: config editing, skills (WYSIWYG + history),
  memory, MCP with live connection testing, and version history with one-click
  revert. Read-only for non-admins.
- **Agents roster redesign**, a toggleable **grid / list** where each agent
  shows only name, **role**, a health dot (up/degraded/down/retired/legacy from
  real container state), and icon controls (start/stop · manage · duplicate ·
  retire/migrate/re-hire). Detail moved into the modal.
- **Editable agent roles** (`agent_defs.role`) — a human title (e.g. "Support
  Lead") shown on the roster, set at creation, editable in the modal.
- **Re-hire + duplicate**, retired agents can be un-retired (re-enable → render →
  start from the preserved volume); any agent can be duplicated into a new one.
  Retire is a typed-slug double opt-in.
- **Fleet Reconcile**, one button renders all managed configs and starts every
  enabled agent that isn't running (drift + cold start; reboot survival is
  already handled by `restart: unless-stopped`).
- **Personal assistants**, everyone can spin up their own Hermes agent (own
  container, key, memory) from Home — `agent_defs.owner_user_id`,
  `createPersonalAgent()`, a "Your assistant" card.
- **Attachments in chat + channels**, images and documents (disk-backed
  `uploads` table, served from `/api/uploads/:id`); images render inline and are
  passed to vision models as data-URL content parts.
- **Per-view access control**, admins grant/revoke each primary view per member
  (`users.denied_views`); denied views are hidden from the nav and route-gated.
- **Audit trail + retention**, a real `audit_log` (actor · action · target ·
  before/after) wired into governance mutations, surfaced to admins on the Audit
  page; `audit_retention_days` is the first admin-editable app setting.
- **Chat tier picker**, the composer's raw model-tier `<select>` is now a
  premium portaled pill.

### Changed
- **Models page** is compact: provider cards show identity + a model count +
  Manage; the model list (with a proper catalog-search **add-model** flow, not a
  LabelPicker), pricing, class, and privacy routing moved into a modal.
- **Modals center on the viewport** (portaled to `<body>`) instead of within a
  backdrop-filtered card.

## [Unreleased]: Phase 5 — product IA + elegance (2026-07-06)

Reworking Talaria from a feature grid into a coherent product: two mental
modes, a real landing, versioned internals, and a self-hostable vocabulary.

### Added
- **Home / Today** at `/`, the seamless landing. In Talaria's guardrail model a
  person's job is to triage, review, and unblock the agents' work, so Home
  surfaces exactly those queues (scoped to your boards) plus unread mentions,
  quick entries into the work surfaces, and an accurate fleet-health glance
  (real container status). Chat moves to `/chat`.
- **Talaria LLM gateway** ([`server/llm-gateway.ts`]), one OpenAI-compatible
  endpoint over the whole model registry (`/api/llm/v1/{models,chat/completions}`,
  streaming). Provider keys stay server-side; per-endpoint `request_defaults`
  merge into every call; usage is metered per key. **Per-user API keys**
  (`tlk_…`, minted in Settings → API keys, admin-grantable via `can_mint_keys`).
- **No-train routing as a setting**, a per-cloud-endpoint toggle on /models
  (OpenRouter no-store allowlist + `data_collection: deny`, or portable deny) —
  privacy is opt-in, not baked in.
- **Versioned skills + memory**, every save is an immutable, authored revision
  (`internal_versions`); recover or load any prior one. Edited in a **WYSIWYG
  modal** (RichEditor + a history rail) — the raw textareas are gone.
- **MCP connection testing**, live status chips (Connected / Login required /
  Unreachable / Error) via a real MCP `initialize` probe carrying the agent's
  identity, plus a premium add-server modal that tests before saving.

### Changed
- **Navigation regrouped** into **Work** (Home · Chat · Channels · Boards ·
  Inbox) and **Manage** (Agents · Models · Compute · Cost · Audit · Alerts) +
  System — simple for the non-technical surfaces, control grouped for the
  technical ones. Skills/Memory/MCP/Models moved off the top nav into the Agents
  page. Fleet overview folded into Agents (`/fleet` → `/agents`).
- **"Local" → "Self-hosted"** in all user-facing copy (people run models on
  local machines *and* other on-prem boxes). "Local inference" → **Compute**,
  "Activity" → **Audit**.

### Removed
- The dead **`/tasks`** nav item (it matched the boards API route; no page ever
  existed behind it).

## [Unreleased]: Phase 2 UI (2026-07-02)

Talaria's own front end ([`ui/`](./ui), Vite + TanStack Start) grows a full,
self-hosted **project-management suite**, owned in Talaria's own Postgres/Redis,
not proxied from mission-control.

### Added
- **Boards & teams**, shareable kanban boards (personal or team-owned), a
  consolidated **Board settings** modal (General / People / Agents), board-scoped
  agent policy (restrictive by default), teams + member management, and soft
  **archive** for boards and tickets.
- **Tickets**, rich detail modal: TipTap WYSIWYG description (markdown under the
  hood) with read/edit toggle + slide-in full-screen editor, syntax-highlighted
  code + hover links, comments (Ctrl+Enter to send), an **Activity** tab, watchers,
  and a quality-review approval gate.
- **Ticket routes**, each ticket is a directly-linkable nested route
  (`/boards/:boardId/:taskId`) with copy-link buttons on cards, list rows, and the
  modal.
- **Fields**, agent-appropriate **effort** (XS-XL), **multiple assignees**
  (board-scoped agents only), ticket **dependencies** (blocked-by / blocks), labels,
  due date, and **auto-accumulated time-spent** (`addTimeSpentSeconds`). Manual hour
  estimates removed.
- **Blocked status**, a new kanban column for stalled / needs-input work.
- **List view**, configurable, drag-reorderable, click-to-sort columns, persisted
  per board.
- **Multiplayer**, live boards via Redis pub/sub → SSE (`/api/boards/:id/events`).
- **Reusable UI primitives**, `CloseButton`, `CopyLinkButton`, `InlineCreate`,
  `danger` button variant; `RichEditor` gains `bare` / `fill` / `onSubmit` modes.
- **Agent guardrails**, on `PUT /api/tasks/:id`, agents may triage but cannot
  self-assign (`assigned` → 403) or self-complete (`done` → `quality_review`), and
  cannot change assignees.
- **Agent MCP (`talaria-mcp`)** ([`mcp/`](./mcp)), an MCP server exposing only the
  safe tools (`list_boards`, `list_tickets`, `get_ticket`, `create_ticket`,
  `triage_ticket`, `comment`, `report_outcome`, `add_time`, `add_dependency`) — no
  assign, no complete; guardrails hold by construction. Identity via
  `TALARIA_AGENT_KEY` + `TALARIA_AGENT_NAME`.
- **Agent-authed task API**, the fleet key plus a new `x-agent-name` header opens
  an agent path on boards list, board tasks (list + create → `inbox`, never
  assigned), ticket detail, comments, and add-dependency. Every agent call is
  checked against the board's agent policy and attributed to the named agent in
  activity/comments. Named agents on `PUT /api/tasks/:id` are policy-checked too;
  unnamed key callers (legacy plugin heartbeat/report) keep their old access.
- **Group chat (channels)**, Slack-style channels where teammates and fleet
  agents are members. Channels + members + agents + messages live in Talaria's
  Postgres; live over Redis pub/sub → SSE (`/api/channels/:id/events`). Agents
  reply when **@mentioned** (by name or model id): the reply streams into the
  channel for every member, built from the channel transcript via the gateway
  plane. Composer has @mention autocomplete; channel settings manage people +
  agents (adding an agent requires access to it). New "Channels" nav surface.

- **People pickers**, one searchable `UserPicker` (over `GET /api/users`, everyone
  who has signed in) replaces every type-an-email field: board sharing, board
  creation invites, teams, and channel members.
- **Label picker**, ticket labels are tag chips + the shared combobox: the board's
  existing labels surface for reuse, and typing creates a new one (Enter or comma).
  Replaces the raw comma-separated text input.
- **Display names**, users can set how they appear (Settings → profile; updates the
  live session, no re-login). Member lists, channel messages, and avatars prefer
  the name and show the email as secondary.
- **Consistent control sizing**, one `sm`/`md` scale (`h-9`/`h-11`) shared by
  Button, Input, Select, and Combobox via a `size` prop — mixed-height form rows
  and hand-set `h-8`/`h-9` overrides are gone.
- **Notifications + user @mentions**, the channel composer autocompletes human
  members alongside agents; @mentioning a person drops a notification in their
  **Inbox** (new nav surface with an unread badge). `GET/PUT /api/notifications`;
  mention tokens are the email localpart, dashed name, or first name.
- **Token ledger**, every agent generation (1:1 chat turn + channel reply) lands
  in `usage_events` — real gateway-reported counts (`stream_options.include_usage`),
  or char-based estimates flagged `~` when the gateway doesn't report. The `/cost`
  page is live: today/7d/30d token tiles, a 14-day daily strip, and a per-agent
  breakdown. Dollar cost lands with per-LLM pricing attribution (see ROADMAP).
- **Admin console**, `/admin` is live: everyone who has signed in, with role
  management (member/admin; `AUTH_ADMIN_EMAILS` admins are pinned, no
  self-demotion) and the per-person agent allow-list UI (empty = all agents) that
  the access model always supported. Admin-gated `GET/PUT /api/admin/users`.
- **Agent harness (phase A)**, Talaria starts becoming the fleet's source of
  truth: `llm_endpoints` (model backends classed local/cloud — feeds the coming
  cost split), `agent_defs` + immutable `agent_versions` (soul, main model,
  `model_aliases` tiers, fallbacks, plugins, MCP servers — diffable, revertible),
  and an idempotent importer that ingests the existing `ai/orchestration` stack
  (`agents.yaml`, per-agent `config.yaml` + `SOUL.md`). `/agents` grows an
  admin-only **Definitions** panel showing each agent's tiers (local/cloud chips),
  fallback chain, soul, and version. Rendering + spin up/down land in phase B.
- **Agent harness (phase B)**, Talaria renders and runs the fleet: versions
  materialize into a gitignored `fleet/` dir (per-agent `config.yaml` emitted as
  YAML 1.1 so PyYAML sees exactly the original semantics, `SOUL.md`, a generated
  compose that reuses the legacy `ai_hermes-<dept>` volumes so memories survive,
  and the gateway manifest — which the bridge now **hot-reloads**, no restart).
  `/agents` gains live container status and lifecycle buttons: start/stop for
  managed agents, one-click **Migrate** for legacy ones (stop old → render →
  start managed → health-gate). Pilot migrated: `sam-support` runs Talaria-managed
  with memories intact, answering through the gateway.
- **Agent harness (phase C1: config control)**, edit an agent **in-app** — soul,
  main model, alias tiers, fallback chain, all against the endpoint registry —
  and save as a new immutable version; **Save & apply** re-renders and restarts
  the managed container. Reverts re-publish an old payload as a new version
  (history is append-only). Structured edits are merged into the raw Hermes
  config so rendering stays faithful.
- **Local vs cloud in the ledger**, every generation now records the serving
  model's endpoint class (from the agent's current main endpoint) + model id.
  `/cost` shows the 30-day local/cloud share bar, a stacked per-day strip, and a
  per-agent "% local" column — the view for optimizing the small-model/frontier
  mixture.
- **Models tab**, a System-area registry for model backends: one-click presets
  for **every common US provider** (Anthropic, OpenAI, Google, xAI, Meta,
  OpenRouter, Groq, Together, Fireworks, Cerebras, Perplexity, DeepInfra,
  DeepSeek, local Ollama/vLLM) with base URLs and wiring preconfigured — pick,
  name the key env var, done. Local/cloud is **inferred** (never asked for known
  providers; LAN/loopback URL heuristic for custom). Provider marks throughout;
  the provider chooser and the model tier picker are the same searchable
  combobox. Each provider card offers its **live catalog** (server-side
  `/models` fetch, keys never leave the box) so you search what the provider
  actually serves; per-provider **model catalogs** (tag-style add/remove) and
  cloud pricing fields.
  The agent editor's clunky selects are replaced by **one searchable picker**
  over every catalog. Deleting a model or provider that agents still use warns
  with the blast radius and **double opt-ins**, then cascades: each affected
  agent gets a new version with the tier stripped (revertible), re-rendered,
  running managed agents restarted. A model that is some agent's **main** is
  never cascaded — reassign first.
- **Agent harness (phase C2: create/retire)**, spin agents up and down on a
  whim. **New agent** on `/agents`: pick a template (any existing agent — model
  tiers/tools/plugins carry over with every identity reference re-stamped to the
  new slug), Talaria allocates a fresh gateway key into the stack `.env`, writes
  v1 with a starter soul, renders a fresh-chassis service (own state volume),
  starts it, and the bridge picks it up live. **Retire** removes the container
  and drops the agent from the fleet manifest; state volume + version history
  stay.

- **Pricing**, real dollars in the ledger: per-model $/MTok prices live on each
  provider (Models page grid; endpoint-level rates as fallback; Anthropic preset
  ships with official prices), every generation records its serving endpoint,
  and cost computes at read time — editing a price reprices history instantly.
  `/cost` gains a **Cloud spend** tile (30d + today), per-model $ in the split
  legend, a per-agent $ column, and a loud warning for **unpriced** cloud tokens
  (never silently $0). Local tokens are $0 by definition.
- **Model-tier routing**, chat any agent on any of its configured tiers: the
  fleet manifest now carries one gateway entry per alias (`<base>-<alias>`,
  resolved by the agent's own Hermes gateway), `/api/agents` returns real
  agents (not raw gateway models) with their tiers, the chat composer gains a
  tier select, and the API validates tiers against the agent's definition. The
  ledger attributes tier-routed turns by the **alias's** endpoint — a `glm`
  turn lands as cloud/glm while main-model turns stay local.
- **Auto-fetched pricing**, zero-config rates: a server-side price oracle pulls
  OpenRouter's public model catalog (no key) and prices every matched cloud
  model automatically (`llm_endpoints.auto_prices`, refreshed in the background
  and on provider/model changes). Cost coalesces user override → auto → endpoint
  default; the pricing grid shows an "auto" tag with fetched rates as
  placeholders. No exact match → honestly unpriced, never guessed.
- **Channel tier mentions**, `@Dex:deepseek` routes that reply to the tier; the
  composer autocompletes `Label:tier`; unknown tiers fall back to main; the
  ledger attributes the turn to the alias endpoint.
- **Activity** (`/activity`), one merged, user-scoped feed — ticket events,
  channel messages, agent config versions — with kind filters. A read model
  over existing tables; nothing new stored.
- **Alerts** (`/alerts`), live-derived health: down/unhealthy managed
  containers, unreachable gateway plane, unpriced cloud usage,
  estimate-dominated ledger, failed and week-stale blocked tickets. Severity
  ranked, deep-linked, nothing to configure.
- **Skills** (`/skills`), the fleet's skills as they exist on disk (shared
  stack dir + each agent's dept/fleet mount): parsed descriptions, live
  SKILL.md editing, admin create/delete. Hermes reads skills per invocation,
  so edits apply on the next run — no restart.
- **Memory** (`/memory`), each managed agent's `memories/MEMORY.md` read and
  written through its running container — no second copy to drift.
- **MCP** (`/mcp`), per-agent MCP servers from the versioned config: add and
  remove as NEW immutable config versions (optionally applied live), untouched
  entries preserved byte-for-byte; plus a talaria-mcp explainer.
- **Inference** (`/inference`), your own hardware: local backends probed live
  (status, latency, serving-now models) plus local token throughput.
- **Per-ticket token spend**, agents report tokens burned on a ticket
  (`POST /api/tasks/:id/usage`, MCP tool `log_usage`, board policy enforced,
  tier-aware); reports are first-class priced ledger rows; the ticket rail
  shows tokens · $ · per-model.
- **Plan chat**, a channel's **Plan** button turns the conversation into
  tickets: a chosen agent (any tier) drafts structured proposals from the
  transcript, a human edits/prunes them in a review modal and creates the
  keepers — into inbox, never assigned.

### Fixed
- SSE event streams no longer crash the server when a client disconnects before
  the Redis subscriber finishes connecting (unhandled rejection in the
  board/channel event stream).
- Ticket labels no longer require hand-typed comma lists (see label picker).
- Logging in no longer clobbers a user-set display name (the provider identity
  only fills the unfriendly defaults).

### Changed
- Sessions are Redis-backed (opaque sid → `sess:<sid>`), not HMAC cookies.

## [Unreleased]: 0.1.0

Initial working slice: hermes-workspace ↔ mission-control bridge + per-agent adapter plugin.
All milestones below verified live against a running stack on 2026-07-01
([`scripts/verify-stack.sh`](./scripts/verify-stack.sh), all checks pass).

### Added
- **Gateway plane** (`bridge/src/gatewayPlane.ts`), the fleet multiplexer. Fronts the Hermes gateway
  `:8642` for a whole fleet: `/v1/models` = every agent, `/v1/chat/completions` routed by model to that
  agent's real gateway (per-agent key, SSE streamed). One workspace talks to every agent via the model
  switcher. Fleet declared in a manifest (`TALARIA_FLEET`/`TALARIA_FLEET_FILE`). Stood up the Phase-1
  fleet engine as a two-plane runtime (gateway multiplexer + dashboard management bridge).
- **Bridge** (`bridge/`, Node/TS), transparent reverse-proxy of the Hermes dashboard `:9119`.
  - **M1** pass-through: all 164 dashboard routes (incl. OAuth + 4 websockets) proxied byte-for-byte
    to the real dashboard; conductor capability-probe (`GET /api/conductor/missions`) served as
    `200 application/json` so the workspace uses remote dispatch instead of native-swarm.
  - **M2** mission create: `POST /api/conductor/missions {name,prompt}` → mission-control
    `POST /api/tasks`, response shaped for the Conductor.
  - **M3** status round-trip: `GET/DELETE /api/conductor/missions/{id}` poll + cancel, mapping
    mission-control task status → the workspace mission enum. Never forces the Aegis-gated `done`.
- **Fleet board**, the bridge serves the workspace's `/api/plugins/kanban/*` surface from
  mission-control, so the swarm/kanban board becomes a live view of the MC fleet (columns = MC
  statuses, cards = MC tasks, full CRUD; `done` stays Aegis-gated). Toggle `TALARIA_KANBAN_FROM_MC=0`.
  The conductor poll's `lines` are also enriched with the task's header + comment feed.
- **Plugin** (`plugin/talaria/`, Hermes standalone), per-agent mission-control adapter.
  - **M3** register (`POST /api/agents/register`) + opt-in background heartbeat
    (`TALARIA_HEARTBEAT_SECONDS`) that polls `/api/agents/{id}/heartbeat` + reports via
    `PUT /api/tasks/{id}` (toward `quality_review`, never `done`). Safe no-op until configured.
- **mission-control adapter** (`adapter/`), **M4** `HermesAdapter` making Hermes a first-class
  framework in mission-control; PR-ready patch + verification.
- **Stack** (`stack/`), compose wiring workspace + mission-control + bridge on the shared `edge`
  network; `scripts/verify-stack.sh` reproduces the M1-M3 verification end-to-end.
- **Docs**, [`docs/m0-contract.md`](./docs/m0-contract.md) (the M0 contract diff + `:9119` allowlist),
  README with architecture + compatibility matrix.
- **Fleet: both Hermes deployment shapes** (`bridge/src/config.ts`, `gatewayPlane.ts`, `sessions.ts`) -
  a fleet entry now supports (A) separate installs (one gateway per agent) and (B) multiple Hermes
  profiles on one host (each profile's API server on its own port). Optional `profile` / `upstreamModel`
  / `pathPrefix` fields: Talaria rewrites the forwarded `model` to the profile when set and honours a
  profile path prefix on chat + session calls. The UI only ever sees Talaria's exposed model ids.
  `pathPrefix` seeds support for Hermes' emerging single-endpoint profile multiplex (`multiplex_profiles`).
- **Phase 2 UI** (`ui/`), the first slice of Talaria's own front end (Vite + TanStack Start, React 19,
  TypeScript), matching the hermes-workspace stack so its chat components lift cleanly.
  - **Mercury design system** (`ui/src/styles.css`, `ui/src/lib/theme.ts`), hand-rolled Tailwind v4
    tokens, dark (`mercury`) + light (`mercury-light`), violet→magenta neon on Mercury-planet neutrals.
    Reuses hermes-workspace's `--theme-*` token contract to keep component lifts frictionless.
  - **Pluggable auth**, each provider independently enable-able (flag **+** secrets required):
    registry (`ui/src/server/auth/config.ts`), stateless HMAC-signed sessions, **Google OAuth**
    (start + callback) and username/password, with routes `/api/auth/{providers,session,google,
    google/callback,password,logout}`. Login screen renders only the enabled providers. Verified live:
    session round-trip, logout, tampered-cookie rejection, provider gating.
  - Both upstreams vendored under `vendor/` (gitignored) as lift sources.

### Key findings (verified against source)
- The Hermes dashboard has **no** `/api/conductor/*` routes, Talaria *serves* them (adds capability),
  it does not override native behavior. Unset `HERMES_DASHBOARD_URL` → 100% native.
- mission-control gates `done` behind **Aegis** approval; Talaria respects it (human-only Done).
- mission-control has no published image (builds from source, pinned `d09e608`).

### Not yet
- Decomposed/broadcast mission parity (those go workspace-local `:3000`), needs the
  `HERMES_MISSION_API_URL` upstream PR to hermes-workspace.
- Executing pulled work inside the Hermes run loop (heartbeat pulls; in-agent dispatch is next).
- Enabling the plugin on the live PackLedger fleet (staged, not yet `--force-recreate`d).

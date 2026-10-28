# Talaria apps — self-contained apps within the app

Apps extend Talaria with whole new surfaces — a CRM, a support desk, a marketing planner — as
**self-contained TypeScript codebases this instance compiles and runs** without a host rebuild.
Not iframes, not webhooks: an app's views live in the same router, design system, and session as
core surfaces. Authors write TypeScript against `@talaria/sdk` — never Rust.

Three parties build them: Outcrop (official apps, maintained outside core), your own developers,
and the community. Core ships with none enabled.


## Anatomy

```
apps/<slug>/
  talaria.json   manifest — name, icon, version, description, surfaces (+ slug, sdk, triggers)
  app.ts         UI surfaces (Svelte 5) — default-exports defineApp({...});
                 required only when talaria.json declares a surface
  *.svelte       one component per surface (plus whatever else you need)
  server.ts      optional API — defineAppServer(...) → /api/apps/<slug>/*, and the
                 handlers for the manifest's triggers
  mcp.ts         optional agent tools — defineAppMcp(...) → governed MCP server
  harnesses/     optional activity harnesses — one defineHarness per file, scored in Models → Fitness
```

The programming model — `defineApp`, the app server, the document store, MCP tools, harnesses —
is the [SDK docset](./sdk/README.md). Scaffold with `bun talaria app new <slug>`. An app with no
UI (a server or agent tools only) declares no surfaces and ships no `app.ts`.

An app written outside a Talaria checkout — a plugin repo of its own, on a laptop or in a
Developer Agent's sandbox — builds against the **SDK kit** the instance serves
(`GET /api/sdk/kit.tgz`): the SDK's declarations, the host's own build recipe and load probe,
and the `talaria-app` CLI (`scaffold`, `check`, `build`, `eval`). A workbench job on a repo with a
`talaria.json` gets it from `prepare_env`. See
[Building an app outside the host](./sdk/outside-the-host.md). A Developer Agent builds, ships and
maintains one by the `talaria-apps` product skill
([`scripts/skills/talaria-apps`](../scripts/skills/talaria-apps/SKILL.md)): a repo from
`request_repo`, the kit's loop before every pull request, `request_app_install` after a person
merges it, and the job sweep's upkeep after that (Plugins, below).

The manifest's rules — required `name`, the `surfaces` shape, an optional `slug` and an optional
`sdk` version range — are published as [`sdk/talaria.schema.json`](./sdk/talaria.schema.json).
Install and update refuse a manifest that breaks them, with the reason; an installed app whose
manifest breaks them is listed in Manage → Apps as **broken manifest**, never enabled, and can
still be uninstalled.

Every path under `/api/apps/<slug>/` is the app's own. The host's routes about an app live under
`/api/app-host/<slug>/`: `POST …/rebuild`, which Manage → Apps calls to compile and probe an app
before enabling it, `POST …/lifecycle`, which only the api calls during an update, and
`POST …/trigger/<id>`, which only the api's trigger scheduler calls (below); plus the two
loopback-only routes the api's fitness plane reads app harnesses through (below).

## Surfaces

An app declares any combination of three surfaces in `talaria.json`:

| surface | where it appears | who reaches it |
| :--- | :--- | :--- |
| `work` | a tile in the top dock's Apps group → `/x/<slug>`, and every route below it | admins, and members granted the work view |
| `manage` | the manage area behind the dock's gear → `/x/<slug>/manage`, and every route below it | admins, and members granted the manage view |
| `settings` | a Settings tab; one page, no routes below it | follows the manage grant |

The app owns the routes below its surfaces: `/x/<slug>/items/42` mounts the work surface with
`items/42` below it, and `/x/<slug>/manage/fields` the manage surface
([client.md](./sdk/client.md#routes-inside-your-app)). A surface never hard-codes its slug: the
slug is whatever this instance installed the app as, and the SDK's hooks bind to it
(`useApp()`).

## Triggers

An app's code can run with nobody calling it: on a schedule, or when something happens on the
platform. `talaria.json` declares each trigger, and `server.ts` answers it by id
([server.md](./sdk/server.md#triggers)):

```json
{ "name": "Lead scorer", "triggers": [
  { "id": "nightly", "cron": "0 3 * * *" },
  { "id": "on-ticket", "event": "ticket.created" }
] }
```

- **`cron`** is a five-field schedule read in **UTC**: minute, hour, day of month, month, day of
  week, each `*`, a number, a range `a-b`, a step `*/s`, `a-b/s` or `a/s`, or a comma list of
  them; or `@hourly`, `@daily`, `@weekly`, `@monthly`, `@yearly`. Numbers only, no month or day
  names. When both day fields are restricted, a day matches if either does (classic cron). The
  api's scheduler fires a schedule within 30 seconds of its minute. A trigger is armed when the
  scheduler first sees it, so installing a nightly job at noon doesn't fire the 3am it "missed";
  minutes missed while the api was down fire once, for the latest.
- **`event`** names a platform event. There are two:

  | event | when | the handler receives |
  | :--- | :--- | :--- |
  | `ticket.created` | a ticket is created, on any board | `ticketId`, `boardId` |
  | `ticket.moved` | a ticket changes column, however it moved: a drag, promotion by assignment, an agent's status write, a review's rejection | `ticketId`, `boardId`, `from` and `to` (column keys), `closed` (whether `to` is terminal: done, failed, cancelled) |

  These are the two changes the platform already announces internally (work dispatch and the
  watchers' notifications ride the same writes). **An event carries ids, never content**: a
  triggered run has no person behind it, so a ticket's title in the payload would be a read path
  around every board's membership. An app that needs the ticket reads it later, as a person.

**A triggered run acts as the app, with no user.** Its context has the app's store, the secrets
an admin gave the app, and the cause (the schedule's minute or the event). It has no `ctx.user`,
no `ctx.api` (there is no session to call the platform with), and it may not drive an agent:
`ctx.agents.turn` refuses from a trigger, and `ctx.harness` may pin no agent. Model spend in a
triggered run goes to the relay on the app's own key: the app's, under its budget (ledger source
`app`), not any person's.

**Which apps fire.** Only an enabled app with a ready build. Disabling an app, or a failed
build, disarms its schedules; turning it back on arms them afresh rather than catching up. An
update or a reinstall that changes a schedule re-arms it. A run queued for an app that was turned
off before it started is recorded as **skipped**.

**Every run is recorded.** Manage → Apps shows each trigger on the app's row: its schedule (and,
on hover, when it next fires) or its event, when it last ran, and how: the string the handler
returned, or the error in red. **A failing trigger never turns the app off.** A handler that
throws, misses its 10-minute deadline, or isn't in `server.ts` at all is a failed run there, and
the next one runs as usual. Runs are kept for two weeks.

**How it runs.** The api decides when and keeps the record (`app_trigger_runs`); it calls the
host's `POST /api/app-host/<slug>/trigger/<id>`, loopback only with the internal service
credential, and the host runs the handler. The api never loads app code.

## Lifecycle

1. **Install** — the app's codebase lands in `apps/`:
   - from the **marketplace** (Manage → Apps → Discover): one click fetches the repo;
   - from **any https git URL** with a `talaria.json` at its root, at its default branch or a
     ref you name (a branch, tag or commit);
   - or by hand: drop/clone a directory into `apps/`.

   An install from git fetches the one commit into a staging directory, checks `talaria.json`,
   and only then moves it into `apps/<slug>`. The slug is the one the manifest declares, else
   the repository's name without a `talaria-app-` prefix (or an override you give); a slug that
   is taken or reserved is refused, and so is one an update or uninstall is working on. The
   repository, the ref and the commit are recorded (`app_sources`), the repository is also
   recorded in the app's data directory (`app-data/<slug>/source.json`), and the install asks
   the host to build the app and answers with the outcome.

   **Kept data.** When an earlier install left data under the slug (see Uninstall), the install
   adopts it only if it was kept from the same repository. Data kept from another repository,
   or from an install that did not record one, is another app's store and database password:
   the install is refused (409, naming both repositories) until it says what to do —
   `keptData: "adopt"` gives the data to the new app, `keptData: "wipe"` deletes it first.
   Manage → Apps offers both as buttons.

   **Private repositories.** A github.com repository the GitHub connection
   ([WORKBENCH.md](./WORKBENCH.md)) can see is fetched with a token scoped to that one
   repository and to reading its contents — including the private repos `request_repo` creates.
   Another host's private repository needs a **workspace secret** holding an https token whose
   *Spendable at* list names that host; name it in the install (`secret`). The token reaches git
   through `GIT_ASKPASS` for that host only — never in the URL, a command line or `.git/config`
   — and a missing credential is an error, never a prompt.
2. **Build** — this instance compiles the app against the host runtime (`/runtime/rt-*.js`)
   and starts a dedicated Postgres container for its data. Dev loads sources through Vite
   (HMR); production serves the compiled chunk. A failed build stays on that app.
3. **Enable** — Manage → Apps (admin). Enable compiles and loads the app first
   (UI surfaces, `server.ts` `fetch`, `mcp.ts` `tools`); a broken build is refused
   with that error. An already-enabled app that later fails is turned off by the
   boot reconciler — opening the Apps page is a read.
4. **Grant** — apps are **explicit-grant**: enabling one gives members nothing. Admins grant each
   app view per person in Admin → People, or per team in Manage → Teams — the same checklist as
   core Manage views. Each surface is its own grant: the work view (`/x/<slug>`) covers the work
   surface and the routes below it, the manage view (`/x/<slug>/manage`) the manage surface, its
   routes and the settings tab. An app that ships settings without a manage surface is still
   granted through `/x/<slug>/manage`. The app API gateway admits a caller who holds **any** of
   the app's views and passes the app `ctx.access { admin, work, manage, settings }`, so an app
   route behind the manage surface checks `ctx.access.manage` itself
   ([server.md](./sdk/server.md#grants)). The compiled client is served under the same rule.
5. **Update** — Manage → Apps shows where each app installed from git came from (repository,
   ref, commit), checks whether the ref now points at a newer commit (`git ls-remote`), and
   offers **Update** (`PUT /api/admin/apps {app, update: true, ref?}`, admin, audited). The check
   never asks a source read with a workspace secret: a check would spend a use of the secret,
   and the page checks on its own whenever an admin opens it. Such an app shows **update not
   checked**; Update fetches with the secret. An update fetches the new commit into
   `apps/.<slug>.next`, builds and load-probes it there while the live app keeps serving, then
   swaps the two directories in one step (`renameat2`
   `RENAME_EXCHANGE`: `apps/<slug>` is never missing, so the host never mistakes the app for
   uninstalled). Once the new version is live, the one it replaced becomes `apps/.<slug>.prev`
   (the source record's `previousSha`); a failed update leaves `.prev` as it was. A version that
   fails to build or load — before the swap or after it — is rolled back automatically: the app
   keeps running the previous commit and stays enabled, and the answer says why. If swapping the
   previous version back fails too, the answer says it was not rolled back and that
   `apps/<slug>` holds the new commit. An update never touches the app's data. The atomic swap
   needs Linux: elsewhere update refuses before it fetches anything.
6. **Uninstall** — disables the app, deletes its MCP server row and every grant on it, stops and
   removes its database container, and deletes the codebase, any update staging directories,
   its build output, its compose file and its source record. Its **data is kept** — the
   Postgres files, `db.env`, the sealed password and the record of the repository they belong
   to — so reinstalling the same repository under the same slug reattaches it (the install says
   so; another repository has to choose, see Install). With `wipeData: true` the app's store is
   emptied while its database still runs, the database is stopped, and the whole data directory
   is deleted (through a throwaway container when its files belong to the database container's
   user), along with the password. Manage → Apps asks which. Audited, like
   enable/disable/install/update.

7. **Plugins** — an app a Developer Agent built and maintains is a *plugin*: the agent asks
   for it to be installed (`request_app_install`), an admin approves it here, and the install
   records who maintains it, who it is for and which board its maintenance tickets go to. Its
   card shows that, with the plugin's **update policy** (`approve`: every update waits for an
   admin, the default; `auto`) and **Request a change**, which files a ticket for its
   maintainer. Approval installs only — the app lands disabled, and enabling and granting are
   the steps above. The workbench job sweep keeps it maintained: a merged PR raises an update
   approval here (**Plugin updates**), and a failed build, a break after a host upgrade (the
   `sdk` range names the version the app needs) or an eval regression becomes a ticket on its
   board. See [WORKBENCH.md](./WORKBENCH.md#plugins).


## The marketplace

Manage → Apps → **Discover** lists apps from the **catalogs** the operator configures. A catalog
is an https URL serving a JSON index — `{"apps": [{slug, name, repo, description, icon, author,
official, version}]}`, where `repo` is an https git URL — and the list lives in Postgres
(`app_catalogs`), edited on the Discover tab's **Catalogs** panel or at
[`/api/admin/apps/catalogs`](./api/admin.md): add, rename, change the URL, turn off, reorder,
remove. Reads are open to anyone with the Apps view; every change is an admin's and is audited.

- Every enabled catalog is read at once, through the SSRF guard. Each app card names the catalog
  it came from; when two catalogs list the same slug, Discover shows the one higher in the list
  and the lower catalog notes it as shadowed.
- A catalog that cannot be read shows its own error next to it; the others still list their apps.
  Adding a catalog reads it once and says how many apps it lists or why it could not be read.
- **The default is a migration, not a fallback.** Upgrading moved the old single catalog URL
  (`apps_catalog_url`) in as the first catalog, or seeded Outcrop's registry
  (`outcrop-labs/talaria-apps`) when none was set, so nothing an install showed went missing. No
  code ever re-adds it: an operator who removes every catalog gets an empty Discover tab that says
  no catalog is configured, and install from Git keeps working.
- A dev stack can list a catalog served from this machine: `TALARIA_APPS_LOCAL_SOURCES=1` admits an
  `http://` URL to loopback, and the SSRF guard needs that host in `TALARIA_FETCH_ALLOW_HOSTS`.

Official apps are maintained by Outcrop Labs and labeled as such; community apps list their author
and repo.

**Trust:** installing an app adds code to your deployment that runs fully trusted, like the
platform itself. Install only apps you trust; the UI says this wherever code enters the system.
The runtime safety boundary is the user session — see the
[security model](./sdk/README.md#security-model).

## Publishing an app

1. Push the app to a public https git repository with `talaria.json` at the root. Repo naming
   convention: `talaria-app-<slug>` (the prefix strips on install). Check it first with the SDK
   kit's `talaria-app check && talaria-app build && talaria-app eval`
   ([outside-the-host.md](./sdk/outside-the-host.md)): the build is the host's own recipe.
2. Anyone can now install it from its git URL.
3. To appear in an instance's Discover tab, list it in a catalog that instance reads: submit it to
   Outcrop's index (`outcrop-labs/talaria-apps`), or serve your own index and add it as a catalog.

Version bumps ship as git commits; an instance shows **update available** when the installed
ref moves, and an admin updates it in one click (Lifecycle → Update above). Declare `sdk` (for
example `"^1.0"`) so a host whose SDK is outside that range refuses the app with a clear reason
rather than failing to build it.

## Operational notes

- `TALARIA_APPS_DIR` overrides where app codebases live (default: `apps/` beside `ui/`).
- `TALARIA_APP_BUILDS_DIR` holds compiled chunks; `TALARIA_APP_DATA_DIR` holds each app's
  Postgres volume, compose file, `db.env` and the MCP tool catalog its load probe wrote. Both
  default beside the apps dir. Uninstall deletes an app's build output and keeps its data
  directory unless asked to wipe it (see Uninstall above).
- `TALARIA_APPS_LOCAL_SOURCES=1` lets install and update read a repository on this machine
  (`file:///…`, or `http://` to loopback). Off by default and not for production: it exists so a
  development stack can exercise the git install and update path without a remote. `talaria dev`
  passes it from `ui/.env` to the api.
- The api calls the host back to build an installed app and to stage and switch an update, at
  `TALARIA_APP_HOST_URL` (default: loopback on the host's `PORT`; `server-entry.ts` sets it for
  the api it spawns), with the same root-derived service credential `/api/internal/*` takes.
- `/app-builds/<slug>/…` serves only `app.js`, `app.css` and `assets/**` of the app's current
  build, and only to a session holding one of the app's views. The compiled `server.js` and
  `mcp.js` never leave the host.
- Each app gets its own Postgres (a compose project, image digest-pinned when the
  local image has one). The password is sealed in `app_settings` and written to a
  0600 `db.env` — never on `docker run -e` and never derived from `TALARIA_SECRET_KEY`.
  `talaria backup` dumps each app DB into the snapshot. The document store (`ctx.store`)
  is the API; app authors never see a connection string.
- The container is `talaria-appdb-<instance>-<slug>`, where the instance is `TALARIA_WORKTREE`,
  `TALARIA_DEVBOX` or `talaria`. The app side and the Rust api must name the same one, so
  `talaria dev` passes the worktree name to the api. Inside the product image
  (`TALARIA_INSTALL=image`) the host reaches the DB by container name on its own network.
  Dev and a checkout install (`server-entry.ts` on the host) reach it on the loopback port
  the container publishes. `TALARIA_APP_DB_NETWORK` names the network outright: the DB joins it
  with no published port, and the host dials it by container name, so set it only on a host
  that is on that network. A devbox sets it to the box's own network (`devbox-<name>_default`,
  in `docker/devbox.compose.yml`): its app runs in a container, where `127.0.0.1` is that
  container's own loopback and a port the host publishes cannot be reached. The box's name is
  its `TALARIA_DEVBOX`, so two boxes running one app name two databases, and `talaria box rm`
  removes a box's app databases with it.
- An app DB reached on its loopback port runs on docker's default `bridge`, so it takes no
  network of its own: a compose file that names none got a `<project>_default` network made for
  it, one subnet per app from the daemon's ~30 address pools. Nothing dials an app DB over a
  docker network there (the host uses the published port; backup and uninstall use `docker
  exec`). A container started without `--network` can reach it by IP, and still needs its
  password. A DB made before this keeps its network while it runs (docker's restart policy
  brings it back on that network); the next time Talaria itself starts it, after it was stopped,
  it moves to the bridge and its old network is removed once no container names it.

- A throw in an app's UI, server, or MCP is isolated — when it is awaited: the pane shows the
  crash, the rest of Talaria keeps running. The boundary is the exception boundary, not a process
  one (apps run fully trusted, in-process): a detached `setTimeout(() => { throw })` or an
  un-awaited promise that rejects is an unhandled rejection in the host, and `process.exit` or
  an infinite loop is the host's. Await what you start.
- A tool argument may arrive carrying a resolved credential (an agent's `«secret:…»` handle is
  resolved on its way into `mcp.ts`), and the app must not persist it — not in `ctx.store`, not
  in a log, not in a thrown message. The call record Talaria keeps never stores arguments either
  ([mcp.md](./sdk/mcp.md)). A secret whose **Spendable at** list (Admin → Secrets) is not empty
  resolves for an app's tools only if that list names `talaria-app://<slug>`; otherwise the
  handler receives the handle text itself.
- The app's server reads a workspace secret with `ctx.secret(name)` only when the secret's list
  names `talaria-app://<slug>` exactly; a secret with an empty list is not readable by app code.
  Every read counts against the secret's use limit and writes a `secrets.spend` audit row with
  actor `app:<slug>` ([server.md](./sdk/server.md#secrets)).
- App MCP servers appear in Manage → MCP badged **app**. Their tool catalogs come from the app's
  load probe, which checks each tool in `mcp.ts` and names any it leaves out on the server's card
  ([mcp.md](./sdk/mcp.md)). They cannot be deleted or re-pointed there; lifecycle belongs to
  Manage → Apps. An app that is turned off or fails its build keeps its server row and every
  grant on it, marked **app off**, until it serves again; only uninstalling it removes the row.
- Everything an app's users do runs under their own session, the app server's `ctx.api` calls
  included: platform permissions, resource ACLs, and the audit log apply exactly as in core
  surfaces.
- An app's code drives a fleet agent only through `ctx.agents.turn`
  ([server.md](./sdk/server.md#driving-an-agent)), and only as far as whoever started that code
  could: from a person's request, an agent that person may use (never someone else's personal
  assistant); from an MCP tool, only the agent calling the tool, proven by its own key; from a
  trigger, none. The api decides it (`POST /api/internal/apps/agent-turn`), from what the host
  authenticated, never from anything the app says. The turn's spend is booked to the agent under
  ledger source `app`.


## Shipping a harness

An app ships an **activity harness** — a model call Talaria runs on your app's behalf through the
one runner: [harnesses.md](./sdk/harnesses.md), deep contract in [HARNESSES.md](./HARNESSES.md).

**Each file in `harnesses/` registers on its own.** One `defineHarness` default export per
`harnesses/*.ts`. The build compiles them into `harnesses.js`, one chunk per file, and the load
probe checks each one against the rules the platform holds its own harnesses to. A harness that
passes gets a column in **Models → Fitness**, labelled with the app's name, and the eval sweep
replays its `evals` against every candidate model. Your `server.ts` or `mcp.ts` runs it in
production with `ctx.harness.run`, bound to the caller ([harnesses.md](./sdk/harnesses.md#running-one--ctxharness)).

- **It must:** name only guard rules that exist; keep its floor inside `requires` and empty
  unless it refuses; for a JSON harness, list `json` in `requires`, because the floor is derived
  from it; keep fixture names distinct; give a widening a reason; keep a temperature between 0
  and 1, and a JSON harness's `repair` a whole number from 0 to 5. Its id must not be a builtin
  harness's id or one that another enabled app registers; when two apps claim one id, the app
  whose slug sorts first keeps it.
- **A harness file that does not compile, or fails a check,** is skipped. The app still builds
  and keeps running. The reason shows on the app's row in Manage → Apps as "N harnesses
  skipped", with one line per file. The rows are rewritten whenever an app is enabled, disabled
  or rebuilt, so an id one app takes from another shows on the app that lost it.
- **Only enabled apps count.** Disabling an app removes its columns; its row reads "N harnesses ·
  not registered" until it is enabled again.
- **How the sweep reaches it.** The api never loads app code. It reads the list from the app host
  (`GET /api/app-host/harnesses`) and replays each fixture there
  (`POST /api/app-host/<slug>/harnesses/<id>/cases/<name>`). Both routes answer only a loopback
  caller that presents the internal service credential. The replay pins the candidate model and
  writes no `harness_runs` or guard-findings rows. Token spend still reaches the usage ledger,
  as every sweep case does.

The coding harness agents drive in their sandbox is not an extension point: the workbench runs
Oh My Pi, and only Oh My Pi ([WORKBENCH.md](./WORKBENCH.md)).

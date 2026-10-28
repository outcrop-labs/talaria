# The Workbench

The Workbench is how Talaria's agents do **real execution work** — starting with software development — inside sandboxes scoped to their role, under lifecycle rules the platform owns. The persona (a Hermes agent) stays the judgment layer: it reads tickets, plans, communicates, and reviews. The workbench is its power tool.

The routing rule that makes the workbench the only path: **agents never do dev work in chat**. Code changes — writing, modifying, or committing code, however small — require a ticket and a workbench job. When dev work is asked for in a chat, the agent's job is to create or link the ticket, post the chat's instructions to it as comments, and move execution into a workbench; even an explicit out-of-band request routes through a ticket first.

It's a **reproducible methodology**, not a dev-only feature. Every workbench is the same six pieces; the dev workbench is simply the first instance (data, design, publishing, and web-operator workbenches ride the same chassis later):

1. **A sandbox**: the dev overlay (env, state dirs, git identity, memory ceiling) and its coding harness, composed into the agent's container by the fleet renderer.
2. **Scoped credentials** — only what the role touches (per-repo GitHub access for dev), never god tokens.
3. **A governed toolkit** — the workbench MCP: the risky lifecycle (branches, PRs, merges) is platform-owned; agents drive it with tools.
4. **Model roles**: the org's Workbench roles become the harness's own roles; the harness picks between them as it works.
5. **MCP pass-through** — the agent's existing MCP grants, rendered into each harness's native config. Nothing reconnects inside a sandbox.
6. **The audit spine** — every job transition lands in the ticket's activity next to dispatch, judge, and review events.

## The Developer Agent switch

Per agent there is exactly one control, on the agent's Summary tab:

> **Developer Agent: on / off**

On sets the agent up end to end, and nothing else needs wiring: the dev sandbox, **Oh My Pi** as its coding harness, and the Workbench tools (`doctor`, `start_job`, `finish_job`, …). Flipping it rolls the agent so the change lands. The switch is the *only* grant of the Workbench MCP server: the registry derives it from `agent_defs.developer` and ignores assignment or team rows for it, and the MCP page shows the Workbench as "every Developer Agent" with no access controls. Which repos the agent may touch stays an explicit pick, shown under the switch once it is on.

Oh My Pi is the only harness. There is no profile registry, no per-agent harness pick, and no per-agent effort→model table. Models come from the org-wide **Workbench model roles** on /models: `code-standard` is omp's default (the `--model` on every invocation line), `code-light` its `smol` role, and `code-heavy` its `slow` and `plan` roles (rendered as `PI_SMOL_MODEL` / `PI_SLOW_MODEL` / `PI_PLAN_MODEL`). omp moves between them on its own (subagents and small steps on smol, the advisor on slow, plan mode on plan), so a job is not pinned to one model. Unset roles fall down (heavy → standard → light → utility), so a missing slot never strands a job. An agent with a **coding account** signed in runs its harness on that subscription's models instead — see [Coding accounts](#coding-accounts-the-harness-on-your-own-subscription).

## Coding accounts: the harness on your own subscription

By default a workbench harness reaches models through Talaria's gateway on the
workbench credential, metered and attributed. **Coding accounts** are the other
option: sign an agent in to a coding subscription — Claude Pro/Max, ChatGPT
Codex, GitHub Copilot, Gemini, Cursor, Z.AI, and the rest of omp's roster — and
its harness runs on that account instead.

What this does *not* change: the agent itself. The persona driving the harness
keeps the model its agent def configures, through the gateway, metered as
always. Coding accounts move the **harness's** models and nothing else.

The one consequence worth saying out loud: a harness run on a subscription does
not pass through the gateway, so **it does not land in Talaria's ledger**. The
provider's own dashboard is the record of that spend.

### Turning it on

`Admin → Agents → Coding accounts` carries two controls, and they are two on
purpose. The first enables the feature. The second is the **allowlist**: which
services this org permits. "Developers may sign agents in to coding
subscriptions" and "to anything omp supports" are different decisions, and an
org that wants Copilot and Codex but not a personal Claude Max subscription
needs to be able to say exactly that. While nothing is permitted, nobody can
sign anything in.

Removing a service stops its accounts driving any harness immediately — jobs
fall back to the gateway — but **keeps the credentials**, so re-permitting it
needs no fresh sign-in. Throwing a credential away takes someone pressing
*Sign out*.

### Signing in

On the agent's Summary tab, under the Developer Agent switch: one account per
service, per agent, signed in by the agent's own **managers** — the same people
who can change its soul or rotate its secrets. Deliberately not every holder of
`agents.manage`: that permission runs the fleet, and running the fleet is not a
licence to put a subscription behind one particular agent. Picking the same
service again replaces the account rather than adding a second one, even
through a different door — `openai-codex` and its headless device variant are
one subscription reached two ways.

The flows are **omp's own**, run by a bridge beside the api
([`omp-auth/`](../omp-auth/README.md)): nothing in Talaria encodes a provider's
client id, PKCE quirk or token endpoint. Three shapes come out of it, and the
sign-in dialog is one state machine over all three:

- **a link to open** — most authorization-code providers. They redirect to
  `http://localhost:<port>` on the machine running the flow, which is the
  instance. On a dev stack (same machine as your browser) the redirect
  completes the login by itself; on a deployed instance it lands nowhere, and
  you paste the code or the failed redirect URL back instead. That fallback is
  the provider's own, declared in omp's auth rules.
- **a link and a user code** — device-code providers. Approve it and the dialog
  finishes on its own; there is nothing to paste.
- **a question first** — some providers ask for an enterprise domain, a region
  or a login method before they hand over a link.

### Who holds the credential

**This instance does.** The whole credential, refresh token included, seals with
secretbox into Postgres and shows up in `Admin → Secrets` like every other
sealed secret. The agent's omp reads it over omp's own **auth-broker protocol**,
served at `/api/workbench/auth/v1/*` and scoped by the agent's own key — so an
agent reads exactly its own accounts and nothing else.

A snapshot hands the harness an access token with `__remote__` where the refresh
token would be, which is what makes Talaria the only thing that can refresh one:
when the token expires, omp calls back and the bridge runs the provider's own
refresher. A credential the provider declares dead is disabled with its reason,
stops being served, and says "sign in again" in the UI rather than retrying for
ever. `POST /v1/credential` — the protocol's upload — is **refused by design**:
logins are a human action in the UI, and a sandbox that could write its own
credential could grant itself a subscription.

A volume reset cannot lose these, and nothing OAuth-shaped ever reaches an
agent's config files.

### What the sandbox can see

Be plain about the other side of that: **code running in a Developer Agent's
sandbox can read the attached account's access token — or, for an API-key
account, the key itself — through the broker**, exactly as omp does. The
broker is scoped by the agent's key, and anything running as that agent (the
harness, a tool it calls, a script the job checks out and runs) holds that
key. The refresh token never leaves the api, so an OAuth access token that
leaks is a short-lived one the provider expires on its own and Talaria alone
can renew. An API key has no such clock: it is a **long-lived secret the
sandbox can exfiltrate**, and nothing here can take it back once it is out.
Attach only keys you can rotate, and rotate one the moment a job you do not
trust has run against it.

Every read of an account's credential by the sandbox writes an audit row —
`coding.snapshot_read`, actor the agent, target the account id, never a
value — rated to one row per account per ten minutes, so an operator can see
*when* the sandbox could read a credential without the long-polling harness
flooding `Admin → Audit`.

### Which plan runs a job, and on which model

A **plan** is either one of the agent's coding accounts or the Talaria gateway.
The gateway is a *choice*, not only a fallback: an agent with subscriptions
signed in can still be told to run some or all of its coding work through the
gateway, on models the org picked. Three layers decide, narrowest first:

1. **the ticket's pin** — this ticket on that plan, optionally naming one
   model;
2. **the agent's default plan** — what a job that pins nothing gets. No account
   marked default means the gateway;
3. **nothing configured** — the org's Workbench model roles, exactly as before
   this feature existed.

Role picks hang off the **plan**, not the agent, and that is the point: a plan
is a coherent set of models, so swapping plans swaps the set with it instead of
leaving `smol` on the subscription that just ran out. Within a plan, a role it
does not fill inherits its `default`; a plan that names nothing leaves the org's
roles standing.

**The per-ticket pin** is on the ticket, above the workbench jobs, because a
subscription runs out mid-ticket and the fix has to be local: move *this* ticket
onto the other account — and name the model too, since the other plan's flagship
is not always what the first plan's default role would have chosen — without
re-pointing the agent and every other ticket with it.

`start_job` resolves the plan per job and puts it **on the invocation line**:
`--model <provider>/<model>` plus inline `PI_SMOL_MODEL` / `PI_SLOW_MODEL` /
`PI_PLAN_MODEL`. Container env can only ever carry the agent's default (it is
rendered per agent, not per ticket), so the line is what makes a pinned plan —
and an account signed in since the last roll — actually take effect. `doctor`
reports which plan the agent codes on, so an agent debugging an unexpected model
has something to read.

## GitHub, connected once

Admin → Org → **GitHub · Workbench**: connect via a **GitHub App** (recommended — short-lived installation tokens, per-repo installs) or a **fine-grained PAT**. The *Setup guide* modal walks every field on GitHub's actual forms. Secrets seal via secretbox and never render back; status live-verifies.

Connecting grants nothing by itself. **Repo access is an explicit per-agent grant** (toggle chips on the agent), validated against the connection's reachable pool. Per-repo **flow** config sets which branch PRs land on (blank = the repo's default). Talaria's job ends at the pull request: how that branch is then reviewed, promoted or merged belongs to the repository's own branch protection and CI, not to anything Talaria assumes.

**Branch protection on every granted repo is required, not optional.** The per-repo rule (`branches_only`, the base branch, an optional prefix) is asked by a `pre-push` hook rendered into the sandbox, and it refuses the configured base, the repo's default branch and `main`/`master`/`rc` with the rule's own sentence. That is a client-side hook: git skips it on `--no-verify`, and the installation token the sandbox's credential helper hands git — scoped to the one repo and narrowed to `contents: write`, `pull_requests: write`, `metadata: read` — is still a token that can push. The rule tells the agent what the law is and logs the attempt; the repository's own protected-branch settings are what make a base-branch push impossible. A repo a free plan cannot protect is a repo whose base branch a prompt-injected job could reach; grant such repos knowing that.

## The job lifecycle (why git never gets messy)

Agents **never run raw git against origin**. The workbench MCP (a Talaria-owned server in the MCP registry, granted by the Developer Agent switch) owns the lifecycle:

- `doctor`: end-to-end self-diagnosis: the Oh My Pi harness (it runs `--version`, then one short model call through omp to the gateway, from a shell stripped of what Hermes strips from the agent's terminal, so a harness that starts but cannot authenticate reads as broken; the call is skipped, and the answer says so, when the agent's jobs run on a coding subscription instead of the gateway, or when the steps before it have used most of the 300 s Hermes gives an MCP call), GitHub, repo grants, omp's model roles, config paths.
- `list_repos`: the agent's granted repos + omp's model roles.
- `start_job(repo, taskId, effort, plan)`: Talaria cuts `talaria/<ticket-ref>-<slug>` (or, when the repo grant carries a configured branch prefix, `<prefix>/<ticket-ref>-<slug>`, named to satisfy the repo's own branch rules so its pushes are accepted) from the flow's base branch, records the job (one live job per ticket — started, queued or awaiting approval), and returns the repo's plain `https://github.com/<owner>/<repo>.git` clone URL (no credential in it: git asks the sandbox's credential helper at push time), a **per-job workspace** (`/opt/data/workbench/jobs/<id>`, so concurrent jobs never collide), omp's model roles, and the Oh My Pi invocation lines (default model filled in). Effort decides planning and how much RAM the job reserves, not the model. **Plans are required for standard/heavy effort**, post to the ticket as a comment *and* as a markdown artifact, and **heavy jobs wait for human approval** from the ticket's workbench strip before any clone URL exists.
- `prepare_env(jobId)`: sets up the job's dev environment after the clone (see "Dev environments" below).
- `job_status` — your jobs, each with its branch, workdir, clone URL, and the PR's URL and number once one is open.
- `finish_job(jobId, summary)` — verifies the branch has real commits, then opens the PR with a templated ticket-linked body (title from the ticket ref, plan + summary inside, the acting agent named) and records its number on the job. When the branch already has an open PR (a revise round), that PR is kept: the pushes already updated it, the summary is posted on it as a comment, and the ticket gets `workbench PR updated`. `abandon: true` closes out a dead job from any live state; on a resumed job it ends the revise round and puts the job back on its open PR.
- `pr_status(jobId)` — read-only: the job's PR state (open, closed or merged), each check run's and commit status's conclusion with one verdict (`passing`, `failing`, `pending`, `none`, or `unknown` when they could not be read), and the review decision from each reviewer's latest review (`approved`, `changes_requested`, `commented`, `none`). Every call underneath is a GET; it never merges, comments, approves or re-runs. It reads with a token narrowed to `pull_requests`, `checks`, `statuses` and `metadata` (all read), so the GitHub App needs **Checks** and **Commit statuses** read permission for a private repo's CI to show; without them the answer says which permission is missing.
- `request_repo(org, name, why, …)` — asks for a new repository; see "Repo creation" below.
- `request_app_install(repo, ref?, why?, taskId)` — asks for the Talaria app in one of the agent's granted repos to be installed; see "Plugins" below.
- `plugin_status(slug?)` — read-only: a plugin the agent maintains; see "Plugins" below.
- `app_harness_fitness(slug)` — read-only: a maintained plugin's harness columns in Models → Fitness; see "Plugins" below.

**Revise rounds keep their PR.** When a reviewer or the QA judge bounces a ticket, the agent calls `start_job` on the same ticket again. If its newest job for that repo is `pr_open`, the PR is still open on GitHub, and the branch still obeys the grant's branch law, that job is **resumed**: the same row goes back to `started` (or queued, or awaiting approval for heavy effort) with the new plan, the same branch and the same workdir, where the earlier round's checkout usually still is (`finish_job` stops its builds and keeps it). The answer carries `resumed: true` and the PR's URL and number. The agent pushes its fixes to the branch, which updates the PR, and `finish_job` keeps that PR rather than asking GitHub for a second one (GitHub refuses a second open PR for the same head with a 422). Anything else, such as a merged or closed PR, starts an ordinary new job. Talaria still never merges, promotes or sequences branches: it reads the PR and leaves the decision to a person.

The same sweep then runs the **plugin maintenance pass** (see "Plugins" below): it reads each plugin's open PRs, its branch, its build and its harness evals, and writes only to Talaria's own tickets and approvals.

**Teardown is the platform's, not the agent's.** When `finish_job` opens the PR, Talaria stops every process still running in the job's workdir and removes the job's `.talaria/env` (the checkout stays for a revise bounce; the project env comes back with the `prepare_env` that follows the resume). Abandoning removes the workdir outright. The `workbench-job-sweep` (every 10 minutes) covers what never calls a verb: a `started` or awaiting-approval job whose ticket reached a done column or was archived is abandoned, and it and any `pr_open` job on a closed ticket lose their workdir. A job with no ticket has no ticket to close it, so time does: `started` or awaiting-approval and untouched for seven days is abandoned and removed; a job still `queued` after a day is abandoned (it never had a workdir). `workspace_cleared_at` marks a job whose workdir is gone.

A cap refusal and a "no commits yet" refusal name the job id, branch, and workdir — finish or push there; retrying the same call does not change the answer. `jobId` is the uuid `start_job` returned, not a ticket ref. Omitting `effort` means `standard`, and `standard`/`heavy` are refused without `plan`.

Git in a job workdir asks Talaria for a credential. A checkout outside that workdir only gets one when git names the repo — the helper reads `origin` when git omits the path. `git credential fill` prints the token into the transcript; do not run it.

**Attribution:** commits are authored as the agent (`Analyst (Talaria agent) <analyst-engineering@agents.talaria.local>` — provisioned git identity per sandbox), so history and blame show who did the work. API-level actions (branch/PR/merge) show the App's identity; PR footers name the acting agent.

**Persistence:** harness session state (Oh My Pi's agent dir, the npm cache, Playwright browsers) lives on the agent's state volume, surviving restarts and rolls, so a session can be resumed later. The volume is named for the agent's department, and a department is one agent's: no two agents share one (see [ARCHITECTURE.md → The fleet](./ARCHITECTURE.md#the-fleet)).

## Repo creation

An agent that needs a repository that does not exist yet, such as a new Talaria app, asks for it with `request_repo(org, name, description, why, visibility?, appSlug?, taskId?)`. Nothing is created until an admin approves.

- **Where.** Admin → Org → GitHub · Workbench lists the orgs where agents may ask (`repoCreationOrgs`; empty turns the feature off). Creation needs the **GitHub App** connection, installed on that org with the org's **Administration** permission. A personal access token cannot create org repos here, so in PAT mode the panel says so and `request_repo` is refused.
- **What the agent learns first.** `doctor` answers `repoCreation: {possible, mode, orgs, reason}` and a check line, so the agent knows the approved orgs and whether creation can work before it asks. `request_repo` is refused up front, with the reason, for an org off the list, outside App mode, or for an org where GitHub says the App is not installed (or is installed but that installation is not selected in the panel).
- **What the request records.** The org and name, the agent's description and reason, the ticket, `visibility` (`private` unless the agent asks for `public`), the app slug when the repo will hold a Talaria app, and the **responsible user**: the person the agent answered to when it asked (the attribution ladder: the assistant's owner, the person chatting with it, or whoever hired it).
- **The decision.** The pending request shows in the panel and in the approvals inbox. **Create + grant** creates the repo at the visibility the approver picks (the agent's choice is the default) and grants it to the requesting agent. The token comes from the installation GitHub names for that org (`GET /orgs/{org}/installation`), never from whichever installation happens to be first, so a multi-org connection creates each repo with the right org's credential. **Reject** takes an optional reason, which is stored on the request and posted on the requesting ticket.
- **Branch protection.** **Protect default branch** is ticked on every request, and the approver can untick it (`protect: false` on the PUT). Ticked, the approval adds a repository ruleset, *Talaria: protect the default branch*, with the same installation token: it blocks force-push and deletion on the default branch (`~DEFAULT_BRANCH`) and nothing else — no required reviews, no required checks, no bypass list. How the branch is reviewed and merged stays the repository's people's call. GitHub refuses rulesets on a private repo in an org on the free plan, and an App without the repository **Administration** permission cannot write one; either way the repo is still created and granted, and the answer, the request row (`protected`, `protection_note`) and the ticket line say the branch is not protected and why.
- **What the agent learns after.** The approval or rejection is a line on the ticket (an approval's line says whether the default branch is protected), and `list_repos` returns the agent's requests from the last 30 days with their status and the approver's note. An approved repo appears in `list_repos`' granted repos.

The new repo starts with an initial commit. When its protection was refused or unticked, protect its default branch on GitHub as for any granted repo: branch protection is required (see "GitHub, connected once" above).

## Plugins

A **plugin** is an installed Talaria app that a Developer Agent built in a repo it was granted, for a person, and keeps maintaining. The record (`plugins`) names the maintaining agent, the person it is for, the board its maintenance tickets go to (the board of the ticket that started it), the git host (`github`), and the admin's **update policy**. Where its code comes from — repo, branch, installed commit — is the app's source record (`app_sources`, [APPS.md](./APPS.md#lifecycle)); the plugin row links to it and goes with it on uninstall.

- **Asking.** Once the app's PR has merged (a person merges it; Talaria never does), the agent calls `request_app_install(repo, ref?, why?, taskId)`. The repo must be one of its granted repos, and `taskId` is required: the ticket's board takes the plugin's maintenance tickets, and the ticket's first human assignee (else its creator, else the person the agent answers to) is who the plugin is for. A repo already installed here is refused (a merge reaches it as an update), and so is a second pending request for the same repo. The request shows in **Manage → Apps → Installed** and in the approvals inbox (kind `app_install`, an admin's to decide, its text bounded to the ticket's board like `repo_request`).
- **The decision.** **Install** runs the same credentialed install as Install from Git (a read-only token scoped to that repo; [APPS.md](./APPS.md#lifecycle)) at the default branch or the requested ref, then records the plugin. Approval installs and nothing more: the app lands **disabled**, and enabling it and granting its surfaces stay admin steps on its card. Kept data from another repository under the slug answers 409 and leaves the request pending until the admin picks **Adopt it** or **Wipe it first**. **Reject** takes an optional reason. Every outcome is one line on the requesting ticket: installed (the slug, the commit, and whether it built), the install's error, or the rejection and its reason.
- **Reading it.** `plugin_status(slug)` answers the installed commit, the commit its branch points at now (`updateAvailable`), the repo's open pull requests with each one's checks verdict and review decision (the same read `pr_status` makes; the first ten get their checks), whether the app is enabled and its build loads (`app.build`, and `app.broken` when talaria.json is refused — an `sdk` range this host is outside of names the version it needs), and the host's harness report. With no slug it lists the agent's plugins and its install requests from the last 30 days. Every call is a read: it never merges, updates, rebuilds or enables anything, and an agent may read only the plugins it maintains.
- **Manage → Apps.** A plugin's card shows its maintainer, the person it is for, its board and ticket, and its update policy; an admin changes the policy there (`PUT /api/admin/apps {app, updatePolicy: "approve" | "auto"}`, audited). `approve`, the default, means every update waits for an admin's approval; `auto` means the maintenance pass applies it as soon as it sees it.
- **Request a change.** Every plugin's card has **Request a change**: what the person writes becomes a ticket on the plugin's board, assigned to its maintaining agent and created by the person asking (`POST /api/admin/apps/change-request {app, request}`; an editor of that board or an admin). The first line is the title.
- **Its harness evals.** `app_harness_fitness(slug)` reads the plugin's columns in Models → Fitness (`app:<slug>/<harness>`): each swept model's band and reason, the harness's own scores for the most recently swept models, and the host's harness report. Every call is a read; it never runs a sweep.

### The maintenance pass

The `workbench-job-sweep` (every ten minutes) runs one pass over every plugin after its workspace passes. It never merges, pushes, comments on GitHub or edits a repository: its only GitHub calls are GETs, and the only update it ever runs is the one the admin's `auto` policy allows.

- **Pull requests.** For each open PR on a GitHub plugin's repo (the first ten), it reads the checks verdict and the review decision — the same read `pr_status` makes. Failing checks, or a reviewer requesting changes, is a comment on the PR's ticket (the workbench job's ticket, else the plugin's) naming the failing checks and the reviewers, and, on a PR a workbench job opened, the maintainer is put back to work: assigned if it was not, the ticket moved out of review into the board's working column, and offered to dispatch, which starts its session unless one is live or a person stopped it on that ticket. A ticket a person parked elsewhere (intake, Blocked) stays where it is, and a PR somebody else opened is said on the plugin's ticket with nobody put on it. Each failure is reported once: checks once per head commit, a change request once per review, so a fix pushed under a standing change request does not wake the agent again (`plugin_pr_watch`). A closed ticket gets the comment and no resume. A PR that leaves the open list is read once more; a merge is a line on its ticket.
- **Updates.** When the plugin's branch points past the installed commit (a merged PR moved it), the pass raises an **`app_update`** approval (admin-only, bounded to the plugin's board) and says so on the plugin's ticket, naming the merged PRs. One update waits per plugin and follows the branch while it waits. **Update** in Manage → Apps runs the update (`PUT /api/admin/apps/update-requests {id, action}`): fetch, build and probe beside the live app, swap, and roll back if the new version fails. **Reject** takes a reason. On `auto`, the pass applies the update itself. Every outcome is a line on the plugin's ticket, and a commit an admin rejected, or one that failed to apply, is not raised again: the next merge moves the branch.
- **Notices.** A failed build, a talaria.json this host refuses, a break after a host upgrade, and an eval regression each become a ticket on the plugin's board, assigned to its maintainer, with at most one open ticket per plugin per kind (`plugin_notices`).
  - *Build failed*: the app's build fails, or its talaria.json is refused for any other reason.
  - *Host upgrade*: the app last built on one host build and fails on the next, or its manifest's `sdk` range excludes this host's SDK — the ticket names the range it needs and the version this host provides.
  - *Eval regression*: a cell in the plugin's harness columns in Models → Fitness got worse since the last pass (ready → workable or unfit, workable → unfit).

## Dev environments

Agents run unprivileged in a stock image, so the platform sets up each job's toolchain rather than leaving the agent to hand-install rustup and linkers into its home. `prepare_env(jobId)`, called right after the clone:

- **Toolchains through [mise](https://mise.jdx.dev)**, in user space on the persistent volume, so the agent's jobs share one download of each version. A repo's own `mise.toml` / `.tool-versions` is used as is. Otherwise the versions come from the files each ecosystem already keeps: `rust-toolchain.toml` (plus mold when `.cargo/config.toml` links with it), `.nvmrc` / `.node-version` / `package.json` (node, and bun or pnpm from `packageManager` or the lockfile), `go.mod`, `.python-version` / `pyproject.toml` / `uv.lock`, `.ruby-version`, at the repo root or one directory down. Detected tools go to a `mise.local.toml` that `.git/info/exclude` keeps out of commits.
- **OS packages through apt, as root**, only for names the repo lists in `.talaria/workbench.toml`, and only from the image's Debian sources. The agent itself never gets root — and root never reads the repo: the scan that finds those names runs as the agent's own user, reads only plain files whose real path is inside the checkout (a symlink anywhere on the path, or a checkout that is itself a symlink, is reported under `skipped` and never opened), and the apt step runs only after every name has passed validation.
- **The project's env, per job.** A granted repo can carry an env store (KEY/value pairs set beside its branch rules in the workbench admin; values are write-only through the API and sealed at rest like every other secret). It reaches the sandbox in exactly one place: `<workdir>/.talaria/env` for the job working that repo, written as the agent (0600, its own file, piped through the exec's stdin, never a host file or a shell line) by `prepare_env` once the checkout exists (an env file written earlier would leave the workdir non-empty and refuse the clone), and removed when the job ends — `finish_job`, abandon, and the sweep all take it; a resumed job gets it again from its next `prepare_env`. `start_job`'s rules say so, and `prepare_env`'s answer carries `projectEnv` (the path, the count, and the `set -a; . <workdir>/.talaria/env; set +a` line that loads it). A checkout at the workdir root gets `.talaria/env` added to `.git/info/exclude`, so no `git add` can commit it. A job on repo A never sees repo B's env, and nothing is mounted fleet-wide. Values a model must never read belong in workspace secrets, not here: a project env is, by design, what the agent's build reads.
- **mise's shims on PATH** in the agent's own terminal, so `cargo`, `bun`, `go` and the rest resolve to the repo's pinned versions by directory (run them from inside the repo; outside it a shim answers "No version is set"). Hermes runs terminal commands with `HOME=/opt/data/home`, not the agent user's passwd home (`/opt/data`), so `prepare_env` writes its block into both: `/opt/data/.profile` and `.bashrc` are what Hermes sources when it snapshots a new terminal session (it expands `~` against the gateway's own HOME), and `/opt/data/home/.profile` and `.bashrc` are what `. ~/.profile` reads from a shell the agent opened before the call. The block carries mise's data dir and global config as absolute paths, so it resolves under either HOME. The harness's own `bun` is also linked into `/usr/local/bin` (below), for a shell that reads no profile at all.
- **The SDK kit, for a Talaria app.** A repo with a `talaria.json` at its root is a Talaria app — keyed on that file alone, never on the repo being Talaria's — and `prepare_env` installs the instance's SDK kit for it (a new app's repo, which holds only its README, gets a seed `talaria.json` from the agent first, and `talaria-app scaffold` writes the app around it): fetched by the sandbox as the agent (its own credential, from the container's env) from `GET /api/sdk/kit.tgz`, installed once per kit version on the department volume beside the harness runtime, and `talaria-app` linked into `/usr/local/bin` where the agent's terminal finds it. A repo that pins no node gets one, for the kit's checkers. The answer carries `appKit`. `prepare_env` also records the job's **eval session** — a random key the relay books to the job and its ticket, written to `<workdir>/.talaria/eval-session.json` (0600, git-excluded) — so `talaria-app eval --gateway`, which sends it on the workbench credential, shows on the ticket's usage. `talaria-app check`, `build` and `eval` run the host's own build recipe and load probe in the sandbox: the host never loads unmerged app code. [docs/sdk/outside-the-host.md](./sdk/outside-the-host.md); the agent's procedure is the `talaria-apps` product skill ([`scripts/skills/talaria-apps`](../scripts/skills/talaria-apps/SKILL.md)).
- **The harness's own runtime, provisioned by the platform.** Oh My Pi is a bun program, and the agent image ships no bun. Before `doctor`, `start_job` or `prepare_env` hands the harness out, Talaria makes sure mise is on the agent's volume, pins and installs bun (`talaria_workbench_harnesses::HARNESS_BUN`), and links `bun` into `/opt/data/.local/bin`, a directory the container's own `PATH` carries. That is not the PATH the agent's terminal has: Hermes runs each terminal command from a login-shell snapshot (with `HOME=/opt/data/home`), and Debian's `/etc/profile` resets a non-root PATH to `/usr/local/bin:/usr/bin:/bin`. So the boot hook also links `/usr/local/bin/bun` to it, on every start. It is idempotent and needs no download once done; `doctor` reports it as `runtime` and probes the harness from that same login shell, as the agent's terminal would run it.
- **A shared Rust compile cache.** For any repo with a `Cargo.toml`, sccache becomes cargo's compiler wrapper, with one cache per department at `/opt/data/workbench/harness/sccache` (20 GiB cap). Every job still starts from an empty `target/`, but a dependency any job already compiled comes out of the cache instead of being rebuilt, so only the first job after a dependency change pays for the full graph. It's scoped to the repo through `mise.local.toml`'s `[env]`, never set globally. Opt out with `sccache = false` in `.talaria/workbench.toml`.

```toml
# .talaria/workbench.toml (optional)
apt = ["libssl-dev", "protobuf-compiler"]
sccache = false   # opt a Rust repo out of the shared compile cache

[tools]           # extra mise tools, merged over detection
protoc = "28"
```

It is idempotent and cheap once versions are cached, so agents call it on every job. A missing tool is fixed by declaring it in the repo, never by the agent installing it. Talaria's own `mise.toml` is the example: the same file gives people and agents Rust, Bun, Node, and mold.

## Keeping a job's disk bounded

Job teardown answers *"is this job over?"* — `finish_job` stops the builds, abandoning removes the workdir, and the sweep catches whatever never called a verb. What nothing asked was *"how big is this?"* A job that is perfectly alive — `started`, ticket open, agent working — holds a checkout plus whatever that project's toolchain writes beside it, and a cold Rust `target/` is 4–20 GiB on its own. Ten live jobs in a department is 200 GiB of entirely legitimate work, and the first notice is a build that fails on ENOSPC.

Two things have to be true to fix that for **any** project, not just the ones Talaria happens to know. Somebody has to measure, because growth nothing reports is growth nobody sees until it is an outage. And somebody has to know what is *rebuildable* — the platform cannot guess that `target/` costs a cargo build while `.venv` cost twenty minutes and a private index. Only the project knows that.

### The third rung: reclaim

| | Processes | Build artifacts | Checkout | When |
|---|---|---|---|---|
| **Stop** | killed | kept | kept | `finish_job` — the PR is open, a revise bounce still needs the tree |
| **Reclaim** | untouched | **dropped** | kept | an idle job, or the volume over budget |
| **Remove** | killed | dropped | dropped | abandoned, or the ticket closed |

**Reclaim** deletes only what the project has said it can rebuild. The checkout, the branch, uncommitted edits and `.git` all survive, so a job that is reclaimed and then resumed pays a rebuild and nothing else — and with the department's shared sccache behind it, a Rust rebuild is mostly cache hits rather than a cold graph.

### What a project declares

`.talaria/workbench.toml` is already where a repo tells the workbench about itself, so cleanup joins it rather than earning a second file:

```toml
[cleanup]
# Rebuildable. Dropped from an idle job before its checkout is ever touched.
# These MERGE OVER detection — name what detection would miss.
artifacts = ["target", "dist", ".turbo"]

# Expensive to recreate: a private-index virtualenv, a downloaded model, a
# fixture corpus. Never dropped, whatever detection thinks, whatever the
# pressure. `keep` always wins over `artifacts`.
keep = [".venv", "fixtures/corpus"]

# This repo's ceiling for ONE job's workdir. The platform default applies
# when unset. A job over its ceiling is reclaimed at the next idle check
# rather than waiting for the volume to get tight.
maxWorkdirGib = 40
```

Every path is **repo-relative**. An absolute path, a `..` segment, an empty segment, or `.git` at any depth is refused and reported back through `doctor` — this list ends in an `rm -rf` inside a container, so it gets the same discipline as `job_workdir`: validated in Rust, validated again in the script, and never handed to a shell on trust.

### What a project gets for free

A repo that declares nothing still gets sane behavior, from the same marker scan `prepare_env` already runs:

| Detected | Treated as rebuildable |
|---|---|
| `Cargo.toml` | `target` |
| `package.json` | `node_modules/.cache`, `dist`, `build`, `.next`, `.nuxt`, `.svelte-kit`, `.turbo` |
| `pyproject.toml`, `requirements.txt`, `.python-version` | `__pycache__`, `.pytest_cache`, `.mypy_cache`, `.ruff_cache` |
| `pom.xml` | `target` |
| `build.gradle`, `build.gradle.kts` | `build`, `.gradle` |
| `*.sln`, `*.csproj` | `bin`, `obj` |
| `mix.exs` | `_build` |

What is **absent** matters as much as what is there. `node_modules` itself is rebuildable only by going back to the network, so only its cache is dropped. `vendor/` is often committed and load-bearing offline. A Go build cache lives in `GOCACHE` outside the repo, so it is capped as a department cache instead of reclaimed per job. The rule throughout: *cheap and local* is reclaimable, *slow or remote* is not.

Opting out is one line — `artifacts = []` — and `keep` overrides any single detected entry, and the subtree beneath it, without discarding the rest. Detection is on by default because a project that must opt in mostly will not, and then the disk fills anyway.

### When reclaim fires

Never against a job that might be building. There is no idle *timer* to tune and get wrong: the reclaim script checks `/proc` for a live process whose cwd is inside the workdir — the same predicate `Stop` already uses, inverted — and answers `BUSY` without touching anything. A job compiling at that moment is simply asked again on the next sweep.

The existing ten-minute job sweep now runs two passes. The **status pass** is the old one, asking *"is this job over?"*. The **size pass** asks what it never could:

- **Per-job ceiling.** Every live job (`started`, `awaiting_approval`, `pr_open`) is measured. One over its ceiling — its repo's `maxWorkdirGib`, or the platform's 25 GiB default — is reclaimed, because a single job is not entitled to the whole disk.
- **Volume budget.** Once the filesystem holding the workbench volume is **80%** full, any live job with something to give back is reclaimed, oldest first, whatever its ceiling. This is the backstop for the case the per-job rule cannot help with: several genuinely active jobs, none individually outrageous, that together fill the disk.

Oldest-first matters: the job most likely to be picked up again keeps its cache longest.

Everything in the size pass is best-effort and per job. A container it cannot reach, a workdir with no checkout, a repo with no policy, or a busy build is skipped — never retried in a tight loop, and never allowed to fail the sweep. The platform does not guess on a repo it could not read.

Every reclaim is logged with the job, the paths dropped and the bytes recovered, and the sweep's own summary line carries the total. Putting it on the ticket's activity alongside the other job transitions — where the audit spine already lives, and where a deletion belongs — is not wired yet.

## Work sessions

Dispatch is not a single exchange. When a ticket enters an agent-start column with an agent assigned, Talaria pushes the work and then **keeps the session going** — continuation turns carrying live ticket status, up to 12 turns with 10 minutes of listening each — until the ticket reaches review/blocked/done. Agents work like a developer at a desk: run the harness, read its structured result, steer, test, repeat. UI work is verified **in a real browser** with screenshot evidence — the agent's own Hermes browser tool, which runs in its sandbox and reaches a dev server on localhost (on v0.22.4 that is `browser_exec`, the Browser Use CLI Hermes enables by default in place of the `browser_*` tools, and its `capture_screenshot()` keeps a PNG whether or not the model can see images); Playwright is for a repo's own e2e suite — "a UI change without a browser check is unverified" (see the `workbench-driving` canonical skill).

Behind the session sit the quality gates: plans (and the heavy-effort approval), the **QA judge** (enforcing by default when enabled — revise verdicts bounce the ticket straight back to the agent with the issues, capped at 3 revisions before a human takes over; pass/escalate always reach a human), and human sign-off as the only path to done.

The chat boundary is policy, not habit: agents never write, modify, or commit code from a chat thread, no exceptions for small changes. Dev work requires a ticket and a workbench job — a dev-work request in chat is answered by creating or linking the ticket and moving execution into the workbench, even when a human explicitly asks for an out-of-band change, and dev-work instructions given in chat are captured as comments on the relevant ticket. The rule ships in every rendered soul (the dev-policy header) and in the fleet-wide talaria-toolkit skill.

### Concurrency and queueing

A workbench agent shares **one** container on **this VM**. Conversation-only agents stay cheap and uncapped. Coding work packs against the VM's `MemAvailable`:

- **VM keep-back** — 25% of `MemTotal`, clamped 2–16 GiB (`TALARIA_HOST_RESERVE` overrides). Kernel, docker, Postgres, Redis, and the app keep that RAM. Agents are `oom_score_adj: 500` so the kernel kills them first.
- **Push / pull** — a session or `start_job` is offered when free RAM can take the next job after that keep-back. Otherwise the ticket is **queued** (`work_wait`): the board card and ticket ticker say "queued · N ahead" / "next up — waiting for RAM" with the packing reason; the 60s sweep still starts it when a slot frees. Approving a heavy plan against a full VM toasts the same reason. Runaway guard is 16 jobs, not a desk size.
- **Ceiling** — each workbench container's hard `mem_limit` is `MemTotal − keep-back` (and never above 32 GiB). A leak OOMs **that agent**, not the VM. Reservation grows with live jobs; the ceiling does not shrink to the estimate.
- **Observability** — once-a-minute `docker stats` plus host pressure (`ok` / `tight` / `critical`) and a leak flag (RSS climbing ≥256 MiB/min for 8+ minutes past 2 GiB) on `GET /api/fleet/resources` and the run-detail Resources pane.

Packing floors: **conversation** 768 MiB / 2 GiB; **workbench idle** 2 GiB; **per job** 1 / 2 / 4 GiB (light / standard / heavy).

The 2026-09-17 dogfood freeze — fourteen tickets, eleven jobs, 4 GiB chassis, twenty-six OOM kills — was a hard `mem_limit` plus unbounded pile-up. Packing + a 32 GiB last-ditch ceiling replaces both the 3-job stall and the 8g one-size box.

One known amplifier stays out of packing, tracked separately: ever-growing session contexts (retried tickets can carry millions of tokens). Leftover per-job processes and workdirs are the job teardown's (above).


### Observing a run

The run-detail modal (every "watch the work" affordance opens it) is the full-insight view, one pane per question:

- **Agent** — Hermes replies and its own tool calls, retained turns above the live tail. A terminal call that invokes the coding harness is not shown here.
- **Turns** — each harness exchange: the ask the agent sent, the harness output, and the prose the agent wrote back before it did anything else.
- **Resources** — the agent container's cpu/memory/process sparklines over the run's window, sampled once a minute by the platform (admin-only; the same series feeds Observability → Compute).

What the live stream cannot show on its own: tool RESULTS for tools that execute inside the agent container (the persona's completion frames don't carry them). The **talaria-events** Hermes plugin closes that gap: rendered into every agent, it reports each tool call's name, arguments, and result (clamped, secret-scrubbed at the api boundary) to `POST /api/agents/tool-events`, which lands `toolfull` frames on the live run's watch stream — so results appear live and in the retained transcripts. Correlation is the agent's newest live work session (a documented v1 approximation; exact persona-session pinning is a follow-up).

## The harness: Oh My Pi

The workbench runs one coding harness, **Oh My Pi** (omp), defined once in `api/crates/talaria-workbench-harnesses`. It authenticates through Talaria's gateway on the workbench credential (metered and attributed) unless the agent has a coding account signed in (see above), keeps its config and sessions in `PI_CODING_AGENT_DIR` on the department state volume, reads the agent's MCP grants from a rendered `mcp.json`, and is invoked unattended (`npx @latest`, print/json mode, `--auto-approve`, no TUI).

The workbench credential reaches omp as `TALARIA_WORKBENCH_API_KEY`, and the rendered `models.yml` names that variable as the `talaria` provider's `apiKey` (a bare name: omp reads an `apiKey` that names an env var from the env, and sends anything else as the literal key). Two things decide that shape. Hermes strips its provider names (`OPENAI_API_KEY`, `OPENAI_BASE_URL`, `ANTHROPIC_*`, … — the set is in `api/crates/talaria-agent-secrets/src/hermes_stripped_env.txt`) from every command its terminal runs, and `terminal.env_passthrough` cannot put them back; the key used to ride `OPENAI_API_KEY`, so omp started from the agent's terminal sent no key at all. And omp reads `models.json` only once, converting it into a `models.yml` beside it that it reads from then on, so the render writes `models.yml` itself: a re-rendered model list or key name then reaches omp on its next start instead of never. Claude Code, Codex, OpenCode and Pi are not offered; their skill names are signposts pointing at Oh My Pi.

omp runs via `npx` on the stock image, no custom image required; first use installs into the persistent cache. The agents' built-in browser is that pattern taken literally: its engine is fetched from npm on first use, which makes the chassis's pinned external resolvers (`AGENT_DNS_1`/`_2` in `fleet/.env`) a hard dependency: no DNS, no browser, and nothing in a health check to say so. For instant first-runs, build the **workbench image** (`scripts/build-workbench-image.sh`: Hermes chassis + Oh My Pi + the dev-env baseline; no Playwright browser, which the runtime `PLAYWRIGHT_BROWSERS_PATH` on the department volume would hide anyway) and set `TALARIA_WORKBENCH_IMAGE` in the app's env; Developer Agents render on it.

## Roadmap

The role workbenches: data (real query/notebook execution, publish-gated), design (asset pipeline + browser rendering), publishing (CMS/email with human-gated sends), web-operator (allowlisted browser work with session audit artifacts).

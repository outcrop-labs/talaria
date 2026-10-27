# A plugin, step by step

The tools are your workbench tools (**workbench-driving** covers `start_job`, the harness and
`finish_job` for any repo). This page is what is different for a Talaria app: the arguments,
the answers, and the sentences a refusal comes back with.

## 1. A repo for a new app — `request_repo`

| argument | |
| :--- | :--- |
| `org` | Required. One of `doctor`'s `repoCreation.orgs` |
| `name` | Required. Lowercase and dashes; `talaria-app-<slug>` by convention |
| `why` | Required. What the app is for, for the approver |
| `description` | One line, the repo's description |
| `appSlug` | The app's slug: lowercase letters, digits and dashes, starting with a letter or digit, at most 64. Recorded for the approver; declare the same `slug` in `talaria.json` |
| `visibility` | `private` (the default) or `public`. The approver can change it |
| `taskId` | The ticket. The request and its decision are lines on it |

It answers `{ status: "pending", repo, visibility, appSlug, note }`. Nothing exists until an
admin approves. Approval creates the repo with one initial commit (a README), protects its
default branch against force-push and deletion unless the approver unticks that, and grants it
to you. `list_repos` shows your requests from the last 30 days with their outcome, and an
approved repo among your granted repos. A rejection's reason lands on the ticket.

Refusals, and what to do:

| sentence | do |
| :--- | :--- |
| `org "<org>" is not approved for repo creation (approved: …)` | Pick an approved org |
| `repo creation is off — …`, `repo creation needs the GitHub App connection, …`, `the GitHub App connection is incomplete …`, `the GitHub App is not installed on <org> — …`, `… this connection does not use that installation — …` | An admin's step. `report_gap` once with the sentence; don't retry |
| `appSlug "<s>" is not an app slug (…)` | Fix the slug |
| `you already have a pending request for this repo — a human will decide` | Wait. Never file it twice |

## 2. The job, and the kit — `start_job`, `prepare_env`

`start_job` (`taskId`, `repo`, `effort`, `plan`) and the clone into its `workdir` are any job's.
Then:

1. **A new repo gets a seed manifest first.** `prepare_env` installs the kit only for a repo
   with a `talaria.json` at its root, and a new repo has only its README:

   ```sh
   printf '{"name": "Field Notes", "slug": "field-notes"}\n' > talaria.json
   ```

2. **`prepare_env`** (`jobId`). Besides the repo's toolchains (and a node for the kit's checkers
   when the repo pins none), a Talaria app's answer carries `talariaApp: true` and `appKit`:

   ```json
   "appKit": { "ok": true, "version": "<kit version>", "bin": "/usr/local/bin/talaria-app",
               "evalSession": "<workdir>/.talaria/eval-session.json", "usage": "…" }
   ```

   The kit is fetched from this instance as you, installed once per kit version, and
   `talaria-app` is linked onto your terminal's PATH. `evalSession` is the key that books
   `talaria-app eval --gateway` to this job's ticket; git never sees it. Call `prepare_env` at
   the start of every job: a host upgrade brings a new kit.
   - No `appKit` in the answer: there was no `talaria.json` at the root when it looked.
   - `"appKit": { "ok": false, "error": "the SDK kit could not be installed: …" }`: the instance
     refused or could not be reached. `report_problem` with the sentence.

3. **A new repo: `talaria-app scaffold`.** It writes the app around the seed: every key the seed
   declares is kept, the README is kept, a `.gitignore` gets the kit's lines. `--harness` adds
   a starter harness with a recorded reply. `--name` and `--icon` override the seed's. What it
   writes: [skeleton.md](skeleton.md).

   | refusal | means |
   | :--- | :--- |
   | `talaria-app scaffold: <dir> already holds an app (talaria.json)` | `talaria.json` sits beside the app's other files: it is an app already. Edit it |
   | `… already has <files> — scaffold into an empty directory or a fresh repository` | A file the skeleton would write is there. Scaffold never overwrites |
   | `… is not JSON …` / `… is not a JSON object …` | The seed is broken. Write `{"name": "…"}` |

## 3. The loop — `talaria-app`

From anywhere in the repo (`[dir]` defaults to the nearest directory at or above the current one
that holds a `talaria.json`):

```sh
talaria-app check && talaria-app build && talaria-app eval
```

Every command takes `--json` (the report as JSON), `--verbose` (what the host's code logged, and
every tool's output) and `--slug <slug>` (build as this slug instead of the manifest's). Exit
status: 0 passed, 1 failed, 2 a usage error.

### check

`talaria.json` held to the validator install uses (`installs as "<slug>"`, and with `sdk`, the
SDK version this host serves), a typecheck with svelte-check against the kit's declarations,
and your `*.test.ts` under vitest. `--no-typecheck` and `--no-tests` skip a step. A slug no
install would accept fails here: `no usable slug: declare "slug" in talaria.json, or name the
repository talaria-app-<slug>`.

### build

The host's own build recipe and load probe, reported as Manage → Apps would: the build (its
key, files and size), the surfaces, the server, each MCP tool kept or left out with the reason,
each harness registered or skipped with the reason, and the probe. A failed build or probe fails
here exactly as it would turn the app off on the host. A tool left out or a harness skipped does
not fail the build, here or there; read the report anyway.

An import the host does not share fails by name, even where a `node_modules` nearby would
resolve it:

```
build      FAILED — … "zod" is not shared with apps: an app has no node_modules of its own where it is installed, so a server-side module may import only @talaria/sdk/server (which re-exports zod as z) and node's builtins
```

In a surface the sentence ends `a surface may import only svelte, @talaria/sdk, @lucide/svelte
and @tanstack/svelte-query`.

### eval

Every fixture of every harness that registers, through the host's own replay: the real runner,
graded by the fixture's own `check`.

- **Recorded, the default.** Replies come from `harnesses/recorded/<file>.json` (`<file>` is the
  harness file's name), by fixture name. No model is called. A fixture with no reply is
  `MISSING` and fails the run:
  `no reply recorded in harnesses/recorded/<file>.json — record one: talaria-app eval --gateway --model <id> --record`.
  A reply recorded for a prompt that has since changed still replays, and says so.
- **Through the gateway:** `--gateway --model <id>`. Each turn goes to this instance's relay on
  your workbench key, booked to the job's ticket — never a provider's own endpoint. Run it once
  without `--model` and it names the models the gateway serves. `--record` writes the replies
  into the recording, merged over what was there: commit them, so the next run is offline.
- `--harness <id or file>` and `--case <name>` narrow the run.
- A harness with a `loop` is dry-run: every tool answers through its `simulate`.
- `FAIL` and `ERROR` fail the run. `GAP` (the fixture could not fairly ask) and `REFUSED` do not.

| sentence | means |
| :--- | :--- |
| `talaria-app eval: --record writes what the gateway answered, so it needs --gateway` | Add `--gateway --model <id>` |
| `talaria-app eval --gateway: no gateway to call. …` | Outside a workbench job: pass `--url` and `--key` |
| `talaria-app eval --gateway: name the model to run the fixtures on with --model <id>. The gateway serves: …` | Pick one of those |
| `talaria-app eval: the app does not build, so its harnesses cannot run` | Fix the build first |

## 4. The pull request — `finish_job`

`finish_job` (`jobId`, `summary`) checks the branch has commits and opens the pull request. Say
what the app does, that the loop passed, and what to look at hardest. A person merges it;
Talaria never does. `pr_status` (`jobId`) reads it: `state`, `mergedAt`, `checks.verdict`
(passing, failing, pending, none, unknown) with each run, and `reviewDecision` (approved,
changes_requested, commented, none) with each reviewer's latest review.

## 5. Installed — `request_app_install`

Once the pull request has merged to the default branch:

| argument | |
| :--- | :--- |
| `repo` | Required. `owner/name`, one of your granted repos, with `talaria.json` at its root |
| `taskId` | Required. The ticket you built it for: its board takes the plugin's maintenance tickets, and its person is who the plugin is for |
| `why` | What the app does and who asked, for the approver |
| `ref` | A branch, tag or commit. Omitted means the default branch, which updates then follow: omit it |

It answers `{ requestId, status: "pending", repo, ref, note }`. An admin decides in Manage →
Apps. Approval fetches the repo, checks `talaria.json`, installs and builds the app **disabled**;
enabling it and granting its surfaces are the admin's. The outcome — installed (the slug, the
commit, whether it built), the install's error, or a rejection's reason — is a line on the
ticket. From then on you are its maintainer.

| refusal | do |
| :--- | :--- |
| `repo "<repo>" is not granted to you — …` | An app is installed from a repo you maintain |
| `taskId is required: …` | Pass the ticket |
| `<repo> is already installed here as "<slug>" — a merge to its default branch reaches it as an update, …` | Nothing to ask: the merge raises the update |
| `you already have a pending install request for this repo — an admin will decide` | Wait |

## 6. Upkeep — what the job sweep does, and the read-only tools

Every ten minutes the job sweep reads each plugin. It never merges, pushes, comments on GitHub
or edits the repo.

- **Pull requests.** Failing checks (once per head commit) or a requested change (once per
  review) is a comment on the PR's ticket. On a PR a workbench job opened, the ticket is
  assigned to you if it was not, moved out of review into the board's working column, and
  offered to you. `start_job` on that ticket resumes the same job, branch and PR; push the fix,
  run the loop, `finish_job`.
- **Updates.** When the branch moves past the installed commit (a merge), an admin gets an update
  approval (or the update applies itself on the `auto` policy). It is fetched, built and probed
  beside the live app, swapped in, and rolled back if it fails. The outcome is a line on the
  plugin's ticket.
- **Maintenance tickets**, on the board of the ticket that started the plugin, assigned to you,
  at most one open per plugin per kind: `[plugin <slug>] build is failing`,
  `[plugin <slug>] broke after the host upgrade` (the ticket names the SDK range it needs and the
  version the host provides), `[plugin <slug>] harness evals regressed` (a Fitness band got
  worse). A person's **Request a change** on the app's card is a ticket for you too.

| tool | reads |
| :--- | :--- |
| `plugin_status` (`slug`) | The installed commit and the branch's (`updateAvailable`), the repo's open PRs with their checks and reviews, whether the app is enabled and its build loads (`app.build`, `app.broken`), the host's harness report. Without `slug`: the plugins you maintain and your install requests from the last 30 days |
| `pr_status` (`jobId`) | One job's pull request: state, checks, reviews |
| `app_harness_fitness` (`slug`) | The plugin's harness columns in Models → Fitness: each model's band (ready, workable, unfit, untested, unbound) and reason, and the latest scores |

A plugin you do not maintain answers `you do not maintain "<slug>" — …`.

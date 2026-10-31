---
name: talaria-apps
description: Build, ship and maintain a Talaria app (a plugin) — TypeScript on @talaria/sdk in a repo of its own, checked with the kit's talaria-app before the PR.
---

# Talaria apps

When to use: a ticket asks for a Talaria app — a new one, or a change to one you maintain (a
maintenance ticket, a "Request a change", a pull request that went red). An app is a git
repository with `talaria.json` at its root: TypeScript and Svelte 5 against `@talaria/sdk`,
never Rust, compiled by the instance that installs it. You build it as a workbench job
(**workbench-driving**: drive the harness, never hand-code), check it in your sandbox with the
SDK kit's `talaria-app`, and stop at the pull request. A person merges. An admin installs,
enables and grants.

## The path

1. **A ticket.** No ticket, no app. `get_ticket` first.
2. **A repo.**
   - *New app:* `doctor` → `repoCreation.orgs`, then `request_repo` with `org`, `name`
     (`talaria-app-<slug>`), `why`, `appSlug`, `taskId` (and `visibility`: private unless you
     ask for `public`). An admin approves; the GitHub App creates the repo (one initial commit,
     a README) and grants it to you. Carry on with other work: the decision is a line on the
     ticket, and an approved repo shows in `list_repos`. Never file it twice.
   - *Existing app:* its repo is in `list_repos`; `plugin_status` lists the plugins you maintain.
3. **The job.** `start_job` (`taskId`, `repo`, `plan`), clone into the `workdir`, then:
   - *New repo only:* write a seed `talaria.json` at the root first —
     `{"name": "Field Notes", "slug": "field-notes"}`, the slug you asked for. `prepare_env`
     installs the kit only for a repo with a `talaria.json` at its root.
   - `prepare_env` (`jobId`) installs the kit and puts `talaria-app` on your PATH; its
     answer's `appKit` says which kit. Open a new shell after it, as for any job.
   - *New repo only:* `talaria-app scaffold` (add `--harness` for a starter model call). It
     writes the app around your seed and keeps the README. Then add `"sdk": "^1.0"` to
     `talaria.json`, for the SDK version `talaria-app version` names.
4. **Build it.** Grow from the skeleton ([references/skeleton.md](references/skeleton.md)),
   with the SDK as it is ([references/sdk.md](references/sdk.md),
   [references/harnesses.md](references/harnesses.md)).
5. **The loop, before every `finish_job`**, from the repo root:
   `talaria-app check && talaria-app build && talaria-app eval`. All three pass, or there is no
   pull request. They run the host's own validator, build recipe, load probe and harness replay,
   here in the sandbox; the host never loads unmerged code.
6. **`finish_job`** opens the pull request. A person merges it. Talaria never merges.
7. **After the merge:** `request_app_install` (`repo`, `taskId`, `why`; no `ref`, so updates
   follow the default branch).
   An admin approves in Manage → Apps. The app lands **disabled**: enabling it and granting
   its surfaces stay with admins. Put in your outcome what to enable and whom to grant.
8. **You maintain it from then on** (below).

Every argument, answer and refusal of these steps:
[references/lifecycle.md](references/lifecycle.md).

## The reflexes

**Import only what the host shares.** A surface (`app.ts`, `*.svelte`): `svelte`,
`@talaria/sdk`, `@lucide/svelte`, `@tanstack/svelte-query`. Server code (`server.ts`, `mcp.ts`,
`harnesses/*.ts`): `@talaria/sdk/server` and node's builtins. `z` comes from
`@talaria/sdk/server`. Tests (`*.test.ts`) add `@talaria/sdk/testing` and `vitest`. Anything
else fails, by name: `"zod" is not shared with apps: …`.

**No `package.json`.** The one `package.json` is the kit's own, which `prepare_env` installs
outside your repo. `scaffold` writes none, and nothing in one would reach the app: the host
installs no dependencies, and `build` refuses a package it does not share even when a
`node_modules` nearby would resolve it.

**Model calls are harnesses.** Declare one per `harnesses/*.ts` with `defineHarness` and run it
with `ctx.harness.run`. Never a provider URL, a key, or your own fetch to a model.

**The host did the trust work.** `server.ts` runs after session, enablement and grant:
`ctx.user` is authenticated, `ctx.access` says which surfaces the caller holds, and `ctx.store`
is this app's own Postgres, the only database it sees. Never open `DATABASE_URL`, never
`process.exit`.

**Never spell the slug.** The instance that installs the app decides it; surfaces read it from
`useApp()` and call their own server with `useAppApi()` / `useAppQuery('path')`.

**Secrets stay sealed.** None in the repo. Server code reads one an admin gave the app with
`ctx.secret(name)`. A tool argument may carry a resolved credential: use it, never store, log
or echo it.

**Awaited throws only.** A throw in your UI, server or tool stays in the app when it is awaited.
A detached `setTimeout(() => { throw })` or an un-awaited rejection is the host's crash.

**Talaria stops at the pull request.** Never merge, never push to the default branch, never
write CI or branch rules into the repo, never force a ticket `done`.

## Upkeep

- **A red pull request or a change request.** The job sweep reads your plugins' pull requests
  every ten minutes. Failing checks or a requested change is a comment on the ticket, and the
  ticket comes back to you. `pr_status` (`jobId`) reads the checks and reviews. `start_job` on
  the same ticket resumes the same job, branch and pull request; fix it, run the loop,
  `finish_job` (the pull request is kept and your summary is posted on it).
- **A merge** becomes an update approval for an admin (or applies itself, on the admin's `auto`
  policy). There is nothing for you to call.
- **Maintenance tickets** land on the board of the ticket that started the plugin, assigned to
  you: `[plugin <slug>] build is failing`, `… broke after the host upgrade`,
  `… harness evals regressed`. A person's **Request a change** is a ticket too. Work each as an
  ordinary job on the plugin's repo.
- **Read-only:** `plugin_status` (`slug`; omit it to list yours), `pr_status` (`jobId`),
  `app_harness_fitness` (`slug`). None of them merges, updates, rebuilds or enables anything.

## Quick map

| You need | Reach for |
|---|---|
| A repo for a new app | `request_repo` with `appSlug` → an admin approves |
| `talaria-app` on PATH | a `talaria.json` at the root, then `prepare_env` |
| A new app's files | `talaria-app scaffold [--harness]` |
| Proof before the PR | `talaria-app check && talaria-app build && talaria-app eval` |
| Fresh recorded replies | `talaria-app eval --gateway --model <id> --record` (spend goes to the ticket) |
| The manifest's rules | [references/sdk.md](references/sdk.md#talariajson); the kit's `schema/talaria.schema.json` |
| The exact API | the kit's declarations, `types/src/sdk/{index,server,testing}.d.ts` |
| A model call | [references/harnesses.md](references/harnesses.md) |
| Installed on the instance | `request_app_install` after the merge → an admin approves |
| How a plugin is doing | `plugin_status`, `pr_status`, `app_harness_fitness` |

The kit's root is `$(dirname "$(readlink -f "$(command -v talaria-app)")")/..`;
`talaria-app version` names it and the SDK version it serves.

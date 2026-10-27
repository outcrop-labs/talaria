# Building an app outside the host

An app is usually written somewhere other than the Talaria checkout that runs it: in a
plugin repo of its own (`talaria-app-<slug>`), by a person on a laptop, or by a Developer
Agent in its workbench sandbox. `@talaria/sdk` resolves only inside a host checkout, so the
instance serves the SDK as a **kit**: everything an app is checked and built with, from the
same host build as the instance itself.

```
GET /api/sdk/kit.tgz
```

## What is in the kit

| part | what it is |
| :--- | :--- |
| `types/` | Declarations for `@talaria/sdk`, `@talaria/sdk/server` and `@talaria/sdk/testing`, generated from the host's own SDK source |
| `lib/`, `dist/server/runtime/` | The host's app build recipe (`app-build/build.ts`), load probe (`app-health.ts`), harness registry checks and fixture replay (`app-harnesses.ts`), and the runtime the `@talaria/sdk/server` and `/testing` entries resolve to — bundled to run with no host and no database |
| `src/styles.css`, `svelte.config.ts` | The stylesheet an app's CSS compiles against and the svelte config, as the host builds with them |
| `kit.json` | The kit's version, the SDK version it serves, the shared-module lists, and the exact versions of the packages it installs |
| `schema/talaria.schema.json` | The manifest's [published schema](./talaria.schema.json) |
| `bin/talaria-app` | The CLI below |

**Versioned with the host build.** The version is the host build stamp — the one the host's own
app builds are keyed on — then a digest of the kit's contents: `dev-a962c3625153-4149a895`.
The response carries it as `x-talaria-kit-version` (and `x-talaria-sdk-version`, and an `etag`
that answers `if-none-match` with a 304). The kit installs its packages pinned to the
versions in the host's own `node_modules`, so the svelte compiler that builds an app in the kit
is the one that builds it on the host.

**Who may fetch it.** An enabled agent presenting its own credential (`x-api-key`, with
`x-agent-name`), or a signed-in member. The kit carries no secret, but it names the host's build
exactly, so it is not public.

**Where it comes from.** A production image builds it with the ui (`bun run build` ends with
`bun scripts/build-sdk-kit.ts`) and ships `dist/sdk-kit/`. A host running from source builds it
on the first request, and again whenever the sources it is built from change.

## The CLI

```
talaria-app scaffold [dir]   write a new app (--name, --icon, --harness)
talaria-app check [dir]      talaria.json, a typecheck, the app's own tests
talaria-app build [dir]      the host's build recipe and load probe
talaria-app eval [dir]       every harness fixture, recorded or through the gateway
talaria-app version
```

`[dir]` defaults to the nearest directory at or above the current one that holds a
`talaria.json`. Every command takes `--json` and `--verbose`. Exit status: 0 passed, 1 failed, 2
a usage error.

- **`scaffold`** writes the skeleton `talaria app new` writes (the same source,
  `cli/src/cmd/app/skeleton.ts`) at a repo's root, plus a `.gitignore` and a README. `--harness`
  adds a starter harness under `harnesses/` with a recorded reply for its fixture, so `eval` runs
  offline from the first commit. A README or `.gitignore` the repo already has is kept.
- **`check`** holds `talaria.json` to the host's rules (the same validator install uses, pinned
  to the schema), typechecks with svelte-check against the kit's declarations and the host's
  compiler options, and runs the app's own `*.test.ts` with vitest, `@talaria/sdk/testing`
  included. The typecheck resolves exactly what an app may import: the SDK's three entries,
  `svelte`, `@lucide/svelte` and `@tanstack/svelte-query`. Nothing else resolves — not
  `zod`, not a package you happen to have locally — because nothing else resolves on the host
  either. `z` comes from `@talaria/sdk/server`.
- **`build`** runs the host's build recipe and the host's load probe, and reports what Manage →
  Apps would: the build (its key, files and size), the surfaces, the server, each MCP tool kept or
  left out with the reason, each harness registered or skipped with the reason, and the probe. A
  failed build or probe is what turns an app off on the host, and fails here. A skipped harness or
  a tool left out does not, on the host or here. The build key is the host's: build the same tree
  against the same host build and the key matches.
- **`eval`** replays every fixture of every harness that registers, through the host's own replay
  (the real runner with the model pinned, graded by the fixture's own `check`):
  - **Recorded** (the default). Replies come from `harnesses/recorded/<file>.json`, by fixture
    name and turn. No model is called. A fixture with no reply is reported `MISSING` and fails the
    run; a reply recorded against a prompt that has since changed still replays, and says so.
  - **Through the gateway** (`--gateway --model <id>`). Each turn goes to the instance's relay,
    `/api/llm/v1/chat/completions`, never a provider's own endpoint. `--record` writes the replies
    back to the recording, merged over what was there. Without `--model` the CLI lists the models
    the gateway serves.

  `--harness <id or file>` and `--case <name>` narrow the run. A `fail` or an `ERROR` (the
  contract did not hold) fails the run; a `gap` (the fixture could not fairly ask) and a
  `REFUSED` do not. The sandbox has no capability probes, so a capability is unknown — and
  unknown is not missing: the floor never refuses here.

### What differs from the host

Nothing in the recipe. The kit runs the host's own modules, so the difference is only in where
they read and write: the build goes to a work area outside the app's tree
(`~/.cache/talaria-app/<dir>-<digest>/`), the probe publishes no MCP catalog and touches no other
app, and the host's database is a stand-in with no server behind it — every setting reads as its
default, as on a fresh instance. The host build refuses a bare import it does not share by name
(`"zod" is not shared with apps: …`), and so does the kit, even where the package would resolve
from some `node_modules` above the app.

## The sandbox loop

A Developer Agent's job on a repo with a `talaria.json` at its root gets the kit from
`prepare_env`, keyed on that file and nothing else (never on the repo being Talaria's):

1. **Install.** The sandbox fetches the kit as the agent — its own credential and the app's
   origin are already in the container's environment — and installs it once per kit version on
   the department volume (`/opt/data/workbench/harness/xdg/talaria-sdk-kit/<version>`), shared by
   every job. `talaria-app` goes on the PATH every shell has. A repo that pins no node gets one,
   for the checkers. `prepare_env`'s answer carries `appKit`: the version, the binary, and the loop.
2. **Check before the pull request.** `talaria-app check`, `talaria-app build` and
   `talaria-app eval`, from the repo, before `finish_job`. All of it runs in the sandbox. The host's
   load probe imports app code into the host's own process, so unmerged code is never checked
   there.
3. **Eval spend goes to the ticket.** `prepare_env` records an eval session for the job: a random
   key, written to `<workdir>/.talaria/eval-session.json` (0600, kept out of git), that the relay
   books to the job and its ticket. `talaria-app eval --gateway` sends it as
   `x-talaria-session` on the workbench credential (`TALARIA_WORKBENCH_API_KEY`), so a fixture
   replayed through the gateway shows on the ticket's usage. The workbench key is fleet-wide; the
   session key is what keeps the booking to one job.

Outside a workbench job, `talaria-app eval --gateway` takes `--url <origin>` and `--key <a gateway
key>` (or `TALARIA_URL` and `TALARIA_LLM_KEY`); the spend is that key's.

## By hand

```sh
curl -fsS -H "Cookie: …" https://talaria.example.com/api/sdk/kit.tgz -o kit.tgz   # or x-api-key
mkdir -p ~/.talaria-sdk-kit && tar -xzf kit.tgz -C ~/.talaria-sdk-kit
cd ~/.talaria-sdk-kit && bun install
~/.talaria-sdk-kit/bin/talaria-app scaffold ~/src/talaria-app-notes --harness
cd ~/src/talaria-app-notes && ~/.talaria-sdk-kit/bin/talaria-app check
```

The CLI runs on bun; the checkers use node when the directory has one, and bun otherwise.

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

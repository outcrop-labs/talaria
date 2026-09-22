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

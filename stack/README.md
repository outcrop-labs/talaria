# stack/ — the gateway plane (talaria-bridge)

One containerized seam between Talaria and the running fleet:

- **talaria-bridge** exposes an OpenAI-compatible **gateway plane** on `:8642`
  that multiplexes every agent's own Hermes gateway — `/v1/models` lists the
  fleet (one entry per agent + per model tier), `/v1/chat/completions` routes
  by model name. Talaria's chat/channels speak to agents through it
  (`TALARIA_GATEWAY_URL` in `ui/.env`).
- The routing manifest is **`stack/fleet.json`**, written by Talaria's fleet
  renderer whenever the fleet changes (gitignored — it carries per-agent keys
  at runtime; the durable source of truth is Talaria's database + `fleet/.env`).
- `fleet.example.json` shows the manifest shape for standalone use.

The compose also carries optional **containerized app** services
(`talaria-ui`, `talaria-postgres`, `talaria-redis`) for running Talaria itself
in docker; for development use `scripts/setup.sh` + `scripts/dev.sh` at the
repo root instead.

```bash
docker compose -f stack/docker-compose.yml --env-file stack/.env up -d talaria-bridge
```

> History: this stack once also ran `hermes-workspace` and `mission-control`
> (the Phase-1 cockpit Talaria replaced). Those services are gone; their
> source lives on only as gitignored reference material under `vendor/` for
> lifting ideas.

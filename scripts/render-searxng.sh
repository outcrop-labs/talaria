#!/usr/bin/env bash
# Render docker/searxng/settings.local.yml from the template, substituting the
# per-install secret from ui/.env. Used by scripts/dev.sh; the container
# entrypoint performs the same substitution for its stack
# (docker/entrypoint.sh, render-searxng role).
#
# SearXNG reads its secret from settings.yml, not the environment, and the
# image's own substitution would need the container to rewrite a mounted file.
# So the template is rendered here instead, with the per-install key, into the
# gitignored file compose mounts. Skipped when the render is already current
# (template older than the local file).
set -euo pipefail
cd "$(dirname "$0")/.."

secret=$(grep '^SEARXNG_SECRET=' ui/.env 2>/dev/null | head -1 | cut -d= -f2- || true)
[ -n "$secret" ] || secret="talaria-$(head -c 24 /dev/urandom | od -An -tx1 | tr -d ' \n')"

if [ ! -f docker/searxng/settings.local.yml ] || [ docker/searxng/settings.template.yml -nt docker/searxng/settings.local.yml ]; then
  sed "s|__SEARXNG_SECRET__|${secret}|" docker/searxng/settings.template.yml > docker/searxng/settings.local.yml
  echo "▸ rendered docker/searxng/settings.local.yml"
fi

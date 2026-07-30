#!/usr/bin/env bash
# Build the workbench image (Hermes chassis + preinstalled harnesses +
# Playwright/chromium). Optional: pass a Hermes base image override.
#   scripts/build-workbench-image.sh [hermes-base-image]
set -euo pipefail
cd "$(dirname "$0")/.."
BASE="${1:-${HERMES_IMAGE:-nousresearch/hermes-agent:latest}}"
echo "Building talaria-workbench:latest from ${BASE}…"
docker build -f scripts/workbench.Dockerfile --build-arg "HERMES_IMAGE=${BASE}" -t talaria-workbench:latest scripts/
echo
echo "Done. Activate it:"
echo "  1. Set the dev profile image: PUT /api/workbench {\"slug\":\"dev\",\"image\":\"talaria-workbench:latest\"}"
echo "     (or via the admin API of your instance)"
echo "  2. Re-render + roll the fleet (POST /api/fleet/render, then compose up -d)."

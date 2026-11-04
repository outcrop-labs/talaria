#!/usr/bin/env bash
# Build the workbench image (Hermes chassis + preinstalled harness + the
# dev-env baseline). Optional: pass a Hermes base image override.
#   scripts/build-workbench-image.sh [hermes-base-image]
set -euo pipefail
cd "$(dirname "$0")/.."
# The default base is the chassis's Hermes pin (scripts/chassis.template.yml).
BASE="${1:-${HERMES_IMAGE:-nousresearch/hermes-agent:v0.22.4@sha256:7a69201726bc64db2072a1371557137c23312d29705cda2b8ea2a0214a50a4cf}}"
echo "Building talaria-workbench:latest from ${BASE}…"
docker build -f scripts/workbench.Dockerfile --build-arg "HERMES_IMAGE=${BASE}" -t talaria-workbench:latest scripts/
echo
echo "Done. Activate it:"
echo "  1. Set the dev profile image: PUT /api/workbench {\"slug\":\"dev\",\"image\":\"talaria-workbench:latest\"}"
echo "     (or via the admin API of your instance)"
echo "  2. Re-render + roll the fleet (POST /api/fleet/render, then compose up -d)."

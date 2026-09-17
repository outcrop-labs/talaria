#!/bin/sh
# Refresh globally-installed coding harnesses to npm @latest.
# Non-fatal: a registry blip must not strand an agent that already has a
# working install. Invoke templates also use npx @latest, so a job still
# picks up a newer CLI even if this helper has not run.
set -eu
npm install -g --prefer-online \
  opencode-ai@latest \
  @earendil-works/pi-coding-agent@latest \
  @oh-my-pi/pi-coding-agent@latest \
  || true

# The WORKBENCH image: the Hermes chassis with the dev-env baseline
# pre-baked, so a first job skips the apt and mise downloads. Purely an
# optimization: the stock image works the same way.
#
# Oh My Pi is NOT baked in. Every job runs the version pinned in
# talaria-workbench-harnesses (OMP_VERSION), installed once per department
# volume by the job's provisioning and started through the job's own
# launcher; a global install here would be a second omp nothing runs.
#
# The BASE is not unpinned: it is the same Hermes pin the chassis renders
# (scripts/chassis.template.yml), so a Developer Agent runs the Hermes every
# other agent runs. Bump the two together (docs/ARCHITECTURE.md, "The fleet").
#
# Build:  scripts/build-workbench-image.sh   (tags talaria-workbench:latest)
# Use:    set TALARIA_WORKBENCH_IMAGE=talaria-workbench:latest in the app's
#         env; Developer Agents render on it, then render + roll.
ARG HERMES_IMAGE=nousresearch/hermes-agent:v0.22.4@sha256:7a69201726bc64db2072a1371557137c23312d29705cda2b8ea2a0214a50a4cf
FROM ${HERMES_IMAGE}

# The dev-env baseline prepare_env builds on: a C toolchain and pkg-config
# for native builds, mold for repos that link with it (Talaria does), and mise
# for per-repo toolchains. prepare_env installs any of these on the stock
# image too; baking them in just skips the first-job wait.
RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential pkg-config mold curl ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && curl -fsSL https://mise.run | MISE_INSTALL_PATH=/usr/local/bin/mise sh

# No Playwright layer. UI checks go through the agent's own browser tools:
# Hermes ships a pinned Chromium at /opt/hermes/tools and hands it to the
# agent-browser CLI (which Talaria installs onto the agent's volume before
# each start of an agent with the browser and the terminal on; the Developer
# Agent switch opens the terminal) through AGENT_BROWSER_EXECUTABLE_PATH, in
# this same container. A baked Playwright browser was never seen anyway: this image
# inherits PLAYWRIGHT_BROWSERS_PATH=/opt/hermes/tools, so it installed there,
# and the render points PLAYWRIGHT_BROWSERS_PATH at the agent's own state
# volume (/opt/data/workbench/harness/playwright, on hermes-<department>) at
# runtime. That path is where a repo's own e2e suite installs the browser
# revision ITS Playwright pins, on its first run, and where that agent's
# later runs find it again.

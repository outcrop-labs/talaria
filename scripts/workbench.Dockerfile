# The WORKBENCH image: the Hermes chassis with Oh My Pi and the dev-env
# baseline pre-baked, so first runs are instant instead of paying npx
# downloads. Purely an optimization: the stock image works via npx @latest;
# this one just makes it warm from second zero. The harness versions are
# unpinned so a rebuild picks up upstream; invoke templates also use @latest
# so a running container auto-updates on the next job without a rebuild.
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

# Oh My Pi, the workbench's coding harness, globally installed. Do not pin:
# auto-update is the point. npx @latest on invoke still fetches newer than
# this layer.
RUN npm install -g @oh-my-pi/pi-coding-agent \
    && npm cache clean --force

# The dev-env baseline prepare_env builds on: a C toolchain and pkg-config
# for native builds, mold for repos that link with it (Talaria does), and mise
# for per-repo toolchains. prepare_env installs any of these on the stock
# image too; baking them in just skips the first-job wait.
RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential pkg-config mold curl ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && curl -fsSL https://mise.run | MISE_INSTALL_PATH=/usr/local/bin/mise sh

COPY workbench-harness-update.sh /usr/local/bin/talaria-harness-update
RUN chmod +x /usr/local/bin/talaria-harness-update

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

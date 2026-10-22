# The WORKBENCH image: the Hermes chassis with Oh My Pi and Playwright
# pre-baked, so first runs are instant instead of paying npx
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

# Playwright + chromium with system deps — UI verification is first-class
# dev work. Browsers land in the image; PLAYWRIGHT_BROWSERS_PATH (set by the
# renderer to the persistent volume) still wins at runtime for updates.
RUN npm install -g playwright \
    && npx playwright install --with-deps chromium \
    && rm -rf /var/lib/apt/lists/*

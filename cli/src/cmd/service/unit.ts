// The unit file `talaria service install` writes — a pure renderer, no Ctx,
// no fs. Every path is baked in by the caller (ctx.root is already
// realpath'd, dockerBin is an absolute PATH walk); nothing here touches the
// host, so the whole file is assertion-friendly.
//
// Shape: Type=oneshot + RemainAfterExit=yes is the only spelling where
// "active" means "stack brought up" and `systemctl stop` runs ExecStop.
// After=docker.service orders the STOP side too — our `down` runs before the
// daemon dies. Requires= starts docker at boot even when docker is disabled.
// Restart=on-failure only: systemd rejects Restart=always for a oneshot, and
// the compose services' own restart policies cover the steady state.

export type UnitOpts = { root: string; dockerBin: string; upArgs: string[] }

export function unitText(o: UnitOpts): string {
  const up = [o.dockerBin, 'compose', '-f', 'docker/compose.yml', ...o.upArgs].join(' ')
  const down = [o.dockerBin, 'compose', '-f', 'docker/compose.yml', 'down'].join(' ')
  return [
    '# /etc/systemd/system/talaria.service — installed by `talaria service install`.',
    "# Talaria's production stack is a docker compose project; this unit is only its",
    '# boot/stop handle. Every service in docker/compose.yml also carries',
    '# restart: unless-stopped, so the daemon restarts the containers on its own —',
    '# the unit makes that explicit, gates it on the stack\'s healthchecks (--wait),',
    '# and owns the clean shutdown (compose down) BEFORE docker.service stops.',
    '# Re-run `talaria service install` after moving the checkout; remove with',
    '# `talaria service uninstall`.',
    '[Unit]',
    'Description=Talaria (docker compose stack)',
    `Documentation=file://${o.root}/docs/CONTAINER.md`,
    'Requires=docker.service',
    'After=docker.service network-online.target',
    'Wants=network-online.target',
    '',
    '[Service]',
    'Type=oneshot',
    'RemainAfterExit=yes',
    `WorkingDirectory=${o.root}`,
    '# No --build here on purpose: boot starts what exists, `talaria deploy update`',
    '# rebuilds. Interpolation comes from docker/.env (compose loads it from the',
    '# compose file\'s own directory) — which is why install persists DOCKER_GID there.',
    `ExecStart=${up}`,
    `ExecStop=${down}`,
    'TimeoutStartSec=20min',
    'TimeoutStopSec=2min',
    '# on-failure only: systemd rejects Restart=always for Type=oneshot. The compose',
    "# services' own restart policies cover the steady state; this only retries a",
    '# boot-time up that failed transiently (image pull, first build).',
    'Restart=on-failure',
    'RestartSec=10s',
    '',
    '[Install]',
    'WantedBy=multi-user.target',
    '',
  ].join('\n')
}

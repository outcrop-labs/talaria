# The testing instance

A Talaria instance that runs whatever `testing` is. Features merge into `testing` and are
exercised together there before a snapshot of it is promoted into `rc`; this instance is
where "exercised" happens on a real deploy, with real agents, instead of in a worktree.

- **It tracks `testing`, a few minutes behind.** Every push to `testing` is built, smoke-tested
  and published by CI; the instance pulls it on its own clock.
- **It is disposable.** Its database, its keys and its agents can be wiped at any time (the
  [reset runbook](#migrations-and-the-reset-runbook)). Nothing on it is anyone's data.
- **It is not a performance proxy.** The api is compiled with cargo's dev profile (our crates
  `-O1`, debug assertions and overflow checks on — [RELEASING.md](../RELEASING.md) says what
  that costs), so a slow page here says nothing about a release, and an assertion panic here
  may be one a release build would have wrapped through.
- **Treat the VM as untrusted.** It runs code an admin may merge into `testing` without
  review, in containers that hold the docker socket. Everything in
  [Provisioning](#provisioning-requirements) and [Data rules](#data-rules) exists so that a
  bad merge on `testing` can reach nothing else.

The hostname is the owner's choice; `<host>` below stands for it (default
`testing.outcroplabs.com`, no Cloudflare Access — the app login is the gate).

## The deploy chain

```
merge into testing
  └─ testing-deploy.yml (GitHub Actions, every push to testing)
       api-package    talaria-api:sha-<sha12>-dev           (dev profile, cache ref buildcache-dev)
       build-smoke    the app image, VERSION=testing-<sha12>, booted by scripts/deploy-smoke.mjs
                      — packages: read only
       publish        talaria:testing-<sha12> (immutable), then :testing → it, only while the
                      commit is still testing's tip — no checkout, no repository script
       verify         once TESTING_URL is set: waits for <host>/api/healthz to report it
  └─ the VM: talaria-follow.timer → /usr/local/bin/talaria-follow
       pulls :testing, reads its org.opencontainers.image.revision label, pins the checkout
       (detached HEAD) and docker/.env (TALARIA_CHANNEL=testing-<sha12>) to that revision,
       runs `talaria deploy up` (registry mode)
```

- **[`testing-deploy.yml`](../.github/workflows/testing-deploy.yml)** — its header is the
  reasoning: why the job that writes packages runs no repository code, why no run is
  cancelled mid-build, why `:testing` only moves forward. It is never named `rc-deploy`:
  rc's promotion evidence is matched by workflow name.
- **Image tags.** `ghcr.io/outcrop-labs/talaria:testing` (moving) and
  `:testing-<sha12>` (immutable, one per testing commit). The api package carries
  `sha-<sha12>-dev`. Nothing in this chain touches `:main`, `:latest`, `:nightly` or `:rc`.
- **[`docker/follow/talaria-follow.sh`](../docker/follow/talaria-follow.sh)** — installed by
  copy, so a merge cannot change the follower. The image and the checkout are always the
  same revision, so the compose files never run ahead of the image they configure.
- **The minimum gap.** The follower applies at most once per `FOLLOW_MIN_GAP_MIN` minutes
  (default 10). Every new digest restarts the app and rolls every running agent
  ([UPDATES.md](./UPDATES.md)); with merges minutes apart, an instance that applied each one
  would never hold still long enough to exercise. A newer image waits out the gap and the
  newest one lands. Merge to live is about 10–15 minutes when the api did not change, and
  20–30 when it did (the dev compile).
- **The hold.** `touch ~/.local/state/talaria-follow/hold` (as the `talaria` user) pauses
  deploys for an exercise session; `rm` it to resume. The follower prints `held:` every pass
  while it is there.

## `testing-watch`

[`scripts/hooks/testing-watch.mjs`](../scripts/hooks/testing-watch.mjs) answers "is my merge
live yet" for an agent or a person, from any clone:

```
node scripts/hooks/testing-watch.mjs <pr-number|sha> [--timeout <min>] [--json]
```

It polls `<host>/api/healthz` until the version it reports — `testing-<sha12>` — is a commit
that **contains** the merge (a later commit counts: the follower often skips past one), and
prints one proof line: `testing-watch: live — <url> runs testing-<sha12>, which contains
<target12>`. Exit 2 names why waiting will not help: `unmerged`, `red-deploy` (a testing-deploy
run carrying the commit failed and nothing newer is building — with its URL), `down` (healthz
failing for over 5 minutes), or `timeout` (default 45 minutes). Any other exit is "could not
run", never a pass. It needs a GitHub token (`GH_TOKEN`, `GITHUB_TOKEN` or `gh auth token`)
and the instance's URL (`TALARIA_TESTING_URL`, else the repository variable `TESTING_URL`).
Run it in the harness's background mode.

## The other lane machinery

[`promote-testing.yml`](../.github/workflows/promote-testing.yml) keeps the lane: the sticky
`testing-red` issue (open while testing's latest push CI or its daily full battery is red),
`sync: rc → testing` back-flow pull requests from `backflow/rc-<sha12>` whenever rc has commits
testing lacks, a frozen `promote/testing-<sha12>` snapshot into rc on dispatch
(`gh workflow run promote-testing.yml --ref main -f open=true`), and the weekday readiness
summary. It does nothing until `testing`'s tree carries the file, so it stays dormant until
`testing` is brought up to `rc`.

## Provisioning requirements

One VM for this instance only.

**Hypervisor and CPU**
- KVM or Proxmox, x86_64 only: every image is linux/amd64, and TEI is amd64-only.
- CPU type `host` or x86-64-v3, not kvm64.

**Size**
- Recommended: 8 vCPU, 16 GB RAM, 150 GB disk. Minimum: 4 vCPU, 8 GB, 80 GB.
- The disk must actually be allocated to `/` or `/var/lib/docker`. Fedora's installer defaults
  the root LV to 15 GB, which broke the radhires and outcrop deploys.
- What fills it: Hermes images (up to about 13 GB); the channel's hold below about 15 GB free
  plus 10% of the filesystem; the TEI model cache; Postgres; workbench volumes; the dev api
  package (about 536 MB); about 200 MB per testing image (the follower removes earlier ones).
- Storage must NOT be on the dev host's pool, whose virtual disk stalls at 100% busy. Use
  another host or pool.

**Network isolation (required)**
- **Segment.** An isolated VLAN or guest segment with no route to the production LAN subnets
  (192.168.0.0/24 for outcrop, 192.168.10.0/24 for radhires, and any other) except the gateway
  and DNS.
- **Host-firewall fallback.** If segmentation is not possible, nftables rules that drop
  traffic to those subnets except the gateway, in both the `output` chain and Docker's
  `DOCKER-USER` chain, so containers are covered too.
- **Inbound.** No ports, ever. SSH (22) only from the owner's workstation and the dev host.
- **Outbound 443:** ghcr.io and pkg-containers.githubusercontent.com; Docker Hub (sidecars,
  nousresearch/hermes-agent); huggingface.co; registry.npmjs.org; github.com, api.github.com
  and objects.githubusercontent.com; the LLM provider endpoints.
- **cloudflared:** outbound 7844 TCP and UDP, plus 443.

**OS and software**
- Fedora 44 Server or Ubuntu 24.04 LTS, with systemd.
- Docker Engine with the compose plugin, 2.24 or later, enabled at boot.
- git, curl, jq and nftables. bun 1.4.0 is installed by the bootstrap.

**Access to hand over**
- A dedicated `talaria` user (or sudo to create it), with a known sudo password.
- Key-only SSH, with NEW testing-only keypairs: one for the owner, one for the dev host's
  agents (`~/.ssh/talaria-testing`). Never `~/.ssh/talaria-deploy`, which opens production
  instances.
- The host or IP, reachable from the owner's workstation and the dev host.

**Cloudflare**
- A remotely managed tunnel `talaria-testing`, in the account that owns outcroplabs.com (the
  wrong account gives error 1033). Ingress → `http://localhost:5273`, plus a catch-all 404; a
  proxied CNAME to `<id>.cfargotunnel.com`. The token is handed over out of band and recorded
  in talaria-infra `customers/cloudflare.env`.

## Bootstrap

Once the VM is handed over and `ghcr.io/outcrop-labs/talaria:testing` exists. Work as the
VM's `talaria` user unless a step says otherwise.

1. **Verify the hardware.**
   - `uname -m` must print x86_64.
   - `grep -o -m1 avx2 /proc/cpuinfo` must find avx2.
   - `nproc` must be at least 4, and should be 8.
   - `free -g` must show at least 8 GB, and should show 16.
   - `df -h / /var/lib/docker` must show at least 80 GB free on the docker root. If the root
     LV is 15 GB, grow it with `sudo lvextend -r -l +100%FREE <root lv>`.
   - Sample disk health with `iostat -x 5 3`. Utilisation must not sit near 100% while the VM
     is idle.
2. **Verify isolation before anything else.**
   - `curl -m3 -sS http://192.168.10.6:5273` and `curl -m3 -sS http://192.168.0.74` must both
     fail (timeout or refused).
   - `curl -sSI https://ghcr.io` must succeed.
   - If segmentation is not in place, install the nftables fallback: an
     `inet talaria_egress` table whose output chain drops
     `ip daddr { 192.168.0.0/24, 192.168.10.0/24 }` except the gateway;
     `sudo nft add rule ip filter DOCKER-USER ip daddr { 192.168.0.0/24, 192.168.10.0/24 } ip daddr != <gateway> drop`
     (after Docker is installed); persist both with the distro's nftables service.
   - After Docker is installed (step 3), re-check from inside a container:
     `docker run --rm curlimages/curl:8.10.1 -m3 http://192.168.10.6:5273` must fail.
3. **Install the base packages.**
   - Fedora: `sudo dnf -y install git curl jq nftables dnf-plugins-core`, add the docker-ce
     repo, then `sudo dnf -y install docker-ce docker-ce-cli containerd.io docker-compose-plugin`.
   - Ubuntu: Docker's apt repo plus `docker-compose-plugin`.
   - Then `sudo systemctl enable --now docker`, and check that `docker compose version`
     reports 2.24 or later.
   - If the user is missing: `sudo useradd -m talaria`. Then `sudo usermod -aG docker talaria`
     and log in again.
4. **Configure the Docker daemon.** Write `/etc/docker/daemon.json` with
   `{"dns":["1.1.1.1","1.0.0.1"],"log-driver":"json-file","log-opts":{"max-size":"20m","max-file":"3"}}`,
   then `sudo systemctl restart docker`.
5. **Install bun 1.4.0.** `curl -fsSL https://bun.sh/install | bash -s "bun-v1.4.0"`, then
   check that `~/.bun/bin/bun --version` prints 1.4.0.
6. **Clone the testing branch.**
   `git clone -b testing https://github.com/outcrop-labs/talaria ~/talaria`.
7. **Switch to registry mode.**
   - `echo 'export COMPOSE_FILE=docker/sidecars.compose.yml:docker/compose.yml:docker/compose.registry.yml' >> ~/.profile && . ~/.profile`.
   - Create `~/talaria/docker/.env` containing `TALARIA_CHANNEL=testing`. The follow script
     later rewrites this to the immutable `testing-<sha12>`.
   - Never use checkout-build mode: it bakes main's api (the root Dockerfile's default
     `TALARIA_API_IMAGE`).
8. **Confirm the state dir is empty.** `sudo ls -A /var/lib/talaria 2>/dev/null` must print
   nothing. A fresh state dir gives this instance its own TALARIA_SECRET_KEY, AUTH_SECRET and
   TALARIA_AGENT_KEY.
9. **First boot.** `cd ~/talaria && ~/.bun/bin/bun talaria deploy up --branch testing`. This
   generates POSTGRES_PASSWORD and the S3 pair, writes TALARIA_DEPLOY_BRANCH=testing, pulls
   `:testing` and starts the stack. Then `curl -s localhost:5273/api/healthz | jq .version`
   must print `testing-<sha12>`.
10. **Claim the admin before any public exposure.** From the owner's workstation,
    `ssh -L 5273:localhost:5273 talaria@<vm>`, open http://localhost:5273/claim and claim. The
    first visitor becomes admin.
11. **Back up the secrets.** Copy `/var/lib/talaria/env/generated.env` and
    `~/talaria/docker/.env` into the password manager as "talaria testing". Never copy either
    to or from another instance.
12. **Set up the Cloudflare tunnel** (the owner, or with the owner's token).
    - Create the tunnel `talaria-testing` in the outcroplabs.com account.
    - Ingress: `<host> → http://localhost:5273`, plus a catch-all `http_status:404`.
    - Add a proxied CNAME to `<id>.cfargotunnel.com`.
    - On the VM: `docker run -d --name talaria-cloudflared --restart unless-stopped --network host cloudflare/cloudflared:2026.8.2 tunnel --no-autoupdate run --token <token>`.
    - Record the token off-box.
    - Then add `COOKIE_SECURE=1`, `AUTH_PUBLIC_URL=https://<host>` and `TALARIA_TRUST_PROXY=1`
      to docker/.env and run `bun talaria deploy up`. `curl -s https://<host>/api/healthz | jq .status`
      must print "ok".
13. **Install the boot unit and the follow script.**
    - `bun talaria service install` (boot ordering; it captures COMPOSE_FILE).
    - `sudo install -m 0755 ~/talaria/docker/follow/talaria-follow.sh /usr/local/bin/talaria-follow`.
      It is installed by copy, so a merge cannot change the follower itself; re-copy it
      deliberately when this page says it changed.
    - `sudo cp ~/talaria/docker/follow/talaria-follow.service ~/talaria/docker/follow/talaria-follow.timer /etc/systemd/system/`.
      Edit `User=` and the paths if the home directory differs.
    - `sudo systemctl daemon-reload && sudo systemctl enable --now talaria-follow.timer`.
    - Check `systemctl list-timers talaria-follow.timer`. `journalctl -u talaria-follow -n 50`
      must show either `waiting` or nothing to do, or an `applying testing-<sha12>` followed by
      `deploy up`.
    - To pause deploys during an exercise session: `touch ~/.local/state/talaria-follow/hold`.
      Remove the file to resume.
14. **Create the agent test user.**
    - In the instance UI, create a NON-admin user, for example agent-tester@<test domain>.
    - On the dev host, put the agents' testing-only key at `~/.ssh/talaria-testing` (mode 600)
      and add its public half to the VM's `~talaria/.ssh/authorized_keys`.
    - Test it: `ssh -i ~/.ssh/talaria-testing talaria@<vm> 'docker ps --format {{.Names}} | grep redis'`.
15. **Connect it to GitHub.**
    - `gh variable set TESTING_URL --body https://<host> -R outcrop-labs/talaria`. This turns
      on testing-deploy's `verify` job and `testing-watch`. The `testing` environment is
      created at the cutover.
    - Only if Access fronts the host: set the `CF_ACCESS_CLIENT_ID` and
      `CF_ACCESS_CLIENT_SECRET` secrets on the `testing` environment, scoped to this hostname,
      or add an Access bypass policy for `/api/healthz`.
16. **Verify end to end.**
    - Merge a small pull request into testing, or re-run the latest testing-deploy with
      `gh run rerun <id>`.
    - `move the testing tag`, then `testing instance runs this commit`, must go green.
    - On the dev host, `node scripts/hooks/testing-watch.mjs <pr>` must exit 0 with the live line.
17. **Configure integrations in the instance UI.** LLM provider keys, with spend caps; the
    separate Google OAuth client, test accounts only; the separate testing GitHub App
    (Contents and Pull requests only, no Workflows); email to a sink.
18. **Record the instance** in the owner's live-instances notes and in talaria-infra: host and
    user; the SSH key names (owner and agents); tunnel id and hostname; that it follows
    `testing` through registry mode plus talaria-follow, with a minimum gap of 10 minutes.

## Agent access

Host agents on the dev host hold one key for this VM: `~/.ssh/talaria-testing`, testing-only,
opening nothing else. They exercise the instance as the **non-admin test user** — never an
admin, never the account that claimed the instance. To get a session without a browser they
use the [repo-traps](../.claude/skills/repo-traps/SKILL.md) trap-5 recipe (mint a session
directly in Redis), run over SSH against the testing stack's own Redis container:

```bash
ssh -i ~/.ssh/talaria-testing talaria@<vm> \
  "docker exec talaria-redis-1 redis-cli SET 'sess:<sid>' '<json>' EX 604800"
curl -H 'Cookie: talaria_session=<sid>' https://<host>/api/<route>
```

`<json>` is the test user's record in the trap's field order, with `"role":"member"`; its `id`
and `sub` come from that user's row on this instance. `<sid>` is any unguessable string.

## Data rules

- Its own database, its own encryption key, its own tunnel. Never restore a production backup
  into it, and never restore it into anything else.
- A separate Google OAuth project (publishing status Testing, a test-user allowlist). Never add
  its redirect URIs to the production client.
- A separate GitHub App without the Workflows permission — Contents and Pull requests only.
- Email goes to a sink.
- In GitHub, only testing-scoped secrets: the `testing` environment's `CF_ACCESS_*`, if Access
  is used. Anyone who can push a workflow edit to testing can read that environment. Never a
  production key, the `talaria-deploy` SSH key, a Cloudflare token that can edit DNS, or this
  VM's own SSH key.
- Image tags `testing` and `testing-<sha12>` only.

## Migrations and the reset runbook

The instance migrates forward on every boot, like any other. Because `testing` carries
migrations that may still change before they reach rc, a migration edited after the instance
applied it stops the boot with "migration N changed after it was applied". That, or any need
to wipe the database, is the reset — it deletes all data and keys, which is intended:

1. `sudo systemctl stop talaria-follow.timer`
2. `cd ~/talaria && bun talaria deploy down --volumes`
3. `docker ps -aq --filter network=talaria | xargs -r docker rm -f`
4. `sudo rm -rf /var/lib/talaria`
5. `sed -i 's/^TALARIA_CHANNEL=.*/TALARIA_CHANNEL=testing/' docker/.env && bun talaria deploy up --branch testing`
6. Claim again over the SSH tunnel (bootstrap step 10), recreate the test user (step 14) and
   back up the new generated.env (step 11).
7. `sudo systemctl start talaria-follow.timer`

## Troubleshooting

Start with `journalctl -u talaria-follow -n 100`.

- **`refusing: <sha> is not on origin/testing`** — `:testing` names a commit testing does not
  have, which means testing was rewritten. That should never happen (the branch guards refuse
  it); stop and find out how, before touching the checkout.
- **`refusing: the checkout … has local edits`** — someone edited a tracked file in `~/talaria`.
  The follower never overwrites them: `git -C ~/talaria status`, keep or discard them, and the
  next pass applies.
- **`refusing: … docker/.env is missing`** — the file holds this instance's generated secrets,
  so the follower will not let `deploy up` mint new ones that no longer open the volumes.
  Restore it from the password manager (bootstrap step 11). Without the backup the volumes
  cannot be opened: write a docker/.env holding only `TALARIA_CHANNEL=testing` and run the
  reset runbook.
- **A pull failure** (`docker pull` or `deploy up` failing on the registry) — the next pass
  retries. Check outbound 443 to ghcr.io and pkg-containers.githubusercontent.com, and that the
  `testing-<sha12>` tag exists (`docker buildx imagetools inspect ghcr.io/outcrop-labs/talaria:testing`).
- **Disk.** Hermes holds back below about 15 GB free plus 10% of the filesystem. `df -h /var/lib/docker`;
  `docker system df`. The follower removes earlier `testing-*` images after each apply.
- **A hold left in place.** `held:` in every pass, and testing-deploy's `verify` timing out:
  `rm ~/.local/state/talaria-follow/hold`.
- **testing-deploy is red.** Its run says which job. A red `build + smoke the testing image`
  is the commit (fix it on testing by a pull request); a red `move the testing tag` or api
  package job is usually infrastructure — re-run it.

## The adopted-updater alternative

Instead of the follow script, the instance could hand updates to its own engine
([UPDATES.md](./UPDATES.md)): adopt it with `TALARIA_UPDATE_IMAGE=ghcr.io/outcrop-labs/talaria:testing`,
and have CI drive `apply` with a machine key held as a `testing` environment secret. That
dogfoods blue/green rolls on every merge. The cost: a credential for the instance in GitHub,
the instance reachable from CI, and — the caveat that breaks it silently — **after adoption,
never run `talaria deploy up` or the `talaria.service` unit again**: the compose-owned container
would come back up underneath the updater's slots. The follow script, `bun talaria service
install` and the reset runbook above would all have to go. The pull model is the default for
that reason.

- **A deployed update rolls the running agents with it.** The app already
  rolled itself blue/green and left the fleet on the old containers, so a
  chassis change, a skill, or a gateway fix did not reach a live agent until
  somebody rolled it by hand. Once an image install is actually on the new
  digest — the updater's cutover landed, or an unadopted image (dokploy,
  compose) came up on a digest the fleet has not been rolled for — every
  running agent rolls blue/green, one at a time, the same roll a config edit
  uses. Stopped agents stay stopped. Blue during its drain does not roll the
  fleet onto the image that is about to stop, a restart of the same image is
  not a deploy, and checkout and dev installs do not roll from here. The roll
  is detached from the reconcile tick and held on its own lease, so two
  replicas do not roll the same fleet twice; a roll that dies mid-way does
  not record the digest, so the next tick retries. Verified: the decision is
  pinned (`fleet_roll_digest` — adopted green rolls, blue does not, a restart
  does not, an in-flight app cutover waits), and `bun run api:check` on the
  1.97.1 pin.

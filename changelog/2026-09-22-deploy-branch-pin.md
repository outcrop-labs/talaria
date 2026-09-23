- **`talaria deploy up/update --branch` pins a checkout to one branch.** The
  pin is `TALARIA_DEPLOY_BRANCH` in `docker/.env`. Later updates fetch that
  branch and fast-forward only — they do not `git pull` whatever HEAD tracks,
  and they will not merge or reset a diverged checkout. Unpinned updates stay
  `git pull --ff-only`. A plain `deploy up` does not fetch. Verified: the CLI
  suite covers pin persistence, fetch-then-ff ordering ahead of the image
  pull, refusal of a bad name, die-before-docker on a failed fetch or
  fast-forward, and an unpinned update left unchanged.

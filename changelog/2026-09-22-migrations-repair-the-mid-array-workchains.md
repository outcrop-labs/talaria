- **Migrations: repair the mid-array workchains insert + CI now replays the
  upgrade path.** #394 landed its three workchains statements in the MIDDLE
  of the append-only array; every fresh database (all of CI) applied it
  green, but every deployed instance refused to boot on the roll — the
  checksum guard fired `migration 338 changed after it was applied` and the
  canary (outcrop) came back rolled to e9f08476 within minutes, public
  domain 200 again. Fixed: the workchains statements moved to the END of
  the array (fleet ledgers at 343 rows upgrade cleanly to 348). CI: the
  migrations job now also replays the UPGRADE — baseline array into a
  second scratch postgres, then the PR's array on top of that ledger, plus
  the snapshot check on the upgraded database — which fails exactly where
  the fleet failed (verified by replaying the incident locally: baseline
  343 → broken array → guard, exit 1; baseline 343 → fixed array → applied
  5, total 348, snapshot matches). Dev stacks that ran the broken array
  from a workchains-era branch need a `talaria reset` — no deployed
  database ever applied it. The upgrade baseline is pinned to e9f08476
  (the last array any instance ran) until the first post-repair migrations
  PR flips it back to origin/main.

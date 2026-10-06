- **A capability gap reported five ways can now rank as one.** A gap's identity
  is `board_id | slug(kind)`, and `kind` is the agent's own free-text name for
  the sort of work it was doing. So "cannot access staging DB" and "no staging
  database credentials" are two rows with `seen_count: 1` each — while
  `seen_count` is the ranking signal the Studio's Suggested queue orders by. A
  real, recurring gap reported differently by five agents never rises, which is
  the exact opposite of what the honesty loop promises: *"repeats bump
  seen_count (frequency is ranking signal)"*.

  Before filing, the report is now weighed against the board's open gaps — a
  pick-one over them with an explicit **none of them** option — and files under
  the matched row's signature when there is one. The override is a *signature*,
  not a row id, so the insert path does not change at all: a resolved match
  collides on `signature` and bumps `seen_count` exactly as a repeat of the
  same slug always has.

  **The asymmetry runs the other way from every other site on this port, and
  the floor is the strictest because of it.** A false *split* costs ranking — a
  real gap looks rarer than it is, which is today's behaviour. A false *merge*
  costs the gap entirely: two different problems collapse into one row, the
  second is never seen again, and nothing downstream can tell. So 0.85, versus
  the workflow pass's 0.75, and the question asks for the test that actually
  distinguishes them — *"two reports are the same problem when fixing one would
  fix the other, not merely when they are about the same system"*.

  An exact slug hit needs no judgment and asks for none: the insert already
  collapses it. Every judgment is recorded under the shadow ledger's
  `gap-align` site, including the no-matches and the ones below the floor,
  because an agreement rate over only the merges is not one. With the port off
  — the default — a report files under its own slug exactly as it always has.

  Resolved at the route rather than inside `report_gap`, because `NotifyDeps`
  carries a pool and nothing more while asking a decision model needs
  `AppState`. The resolver itself lives with the concept in `talaria-gaps`;
  only the call sits at the one caller that holds state.

  Verified: 5 tests in `talaria-gaps` (3 new — the merge floor pinned by
  behaviour at its boundaries, including that the workflow pass's 0.75 does
  *not* act here; `none` proven unable to collide with a candidate id, which
  are indices, so a typo cannot read as a merge; and a candidate rendering as
  its kind *and* what was missing, since the model is asked whether two
  problems are the same and a bare slug would ask it to compare labels). The
  candidate query was run against real Postgres on both branches of its board
  filter — null board and a given board id — because a `$1::text is null`
  predicate is the kind of thing that compiles and then returns the wrong set.
  clippy clean; `bun run gate` green.

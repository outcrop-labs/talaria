- **A Google login for an email that already has an account lands on that account —
  no more forked second user.** Sign-in resolved people by sub alone, and subs are
  per-door (`google:<subject>` vs `password:<email>`), so a Google login for a
  claimed email silently inserted a second `users` row with role `member`: the
  person's admin powers stayed on a row Google could never reach, and every Google
  session ran as the fork — `users.email` has no unique constraint, so the insert
  never failed. Sign-in now links by verified email first (Google refuses
  unverified addresses, and the password path already links by email — the same
  trust, the missing half), adopting the Google sub onto the existing row without
  touching its role; a fork the repair hasn't merged yet can't have its sub stolen,
  and the first-claim path promotes a same-email member to admin instead of
  forking. Existing deployments heal at boot: a one-shot migration merges every
  forked pair — admin row survives, everything pointing at the fork is re-pointed
  or dies in its cascade — before the api starts. Merged-away rows' sessions age
  out with the 7-day Redis TTL; those people sign in once more and land on the
  survivor. Deployments need a rebuilt image; the repair runs at boot, no operator
  action.

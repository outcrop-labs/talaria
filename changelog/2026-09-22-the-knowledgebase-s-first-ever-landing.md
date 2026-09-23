- **The knowledgebase's first-ever landing stopped swallowing doc clicks.**
  Land on `/knowledge` with no saved selection and the sidebar rendered the
  first space's tree — but every doc click was a no-op, until you opened a
  second space and came back. Two defects, one scene. The canonicalising
  effect (put the first space in the URL) had returned at `!restored` on its
  first run, and a plain `let` latch can never re-run an effect: on a landing
  with nothing to restore, nothing ever changed the URL, so it stayed bare.
  And on a bare URL a doc click called `setLoc(null, doc)` — which navigates
  to that same bare URL. The latch is `$state` now, so the effect wakes when
  the restore answers, and the restore's answer (`restoredSpace`) keeps the
  first-space default from racing a real saved selection. The doc click
  names its own space — the tree renders under the active space whether or
  not the URL carries one. Verified end-to-end in a browser: first-ever
  landing canonicalises to the first space, a saved selection still restores
  to ITS space, a deleted space's memory still falls to the first space,
  and a doc click on a fresh landing opens the document.

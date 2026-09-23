- **W9a — one members header.** `components/app/MembersHeader.svelte` (190 lines)
  replaces plan's and research's copies; both routes become wiring (42 and 55
  lines). The seven real differences are props: `chipSurface` (`bg-raised` vs
  `bg-card`), `active` (presence rings), `hideWhenOwnerless`, `loading`, `noun`
  (the copy), `directory`, and the `meta`/`empty` snippets. The error-message
  dialect stays per route on purpose (`(e as Error).message` vs `errorMessage(e)`
  is user-visible text). Both routes now read the shared teams directory. Parity
  was checked as a class-token multiset: the only tokens that differ are the four
  chip colour strings the prop now supplies.

// Mercury is matte: the backdrop is the flat ground surface — no grids,
// no glows, no drifting blooms. Kept as a component so existing layout
// imports (_app.tsx, login, join) stay stable.
export function MercuryBackdrop() {
  return <div aria-hidden className="pointer-events-none fixed inset-0 -z-10 bg-surface" />
}

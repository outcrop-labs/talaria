// The dither engine's door, kept where every ui/ importer already points
// (DitherLayer, Panel, EmptyState, NavRail, StreamText, TicketGhost, BriefHero,
// Generating, dither-surface). The engine itself lives in `dither-engine.ts`,
// which imports nothing, so the desktop launcher — a separate package — can
// alias that one file instead of keeping a 649-line copy in step by hand.
export * from './dither-engine'

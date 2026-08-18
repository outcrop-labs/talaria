// Generation-in-progress — Mercury motion grammar (spec §9). Loaders are
// rows of small rounded bars (3×12px, like the chip meters) on the gd-*
// motifs: weave (submitting burst), scan (agent stage / tool activity),
// breathe (ambient background monitor), helix (reasoning / generating loop),
// pulse (two-state fallback — also the reduced-motion swap, handled in
// styles.css). Calmer than a spinner, honest about indeterminate work. Use
// `lines={0}` for just the status line.
//
// WHAT THIS FAMILY STILL OWNS, after the waiting marks landed: the BARS. A row
// of bars is shaped like the text that is coming — it is a promise about the
// RESULT, which is why it belongs in a block that the result will replace. The
// activity MARK that says "an agent is working right now" is no longer here; it
// is `WaitingMark`, dealt per session from the thirty states in `lib/waiting/`.
// `Generating` renders one on its status line and takes a `site` to say which.
//
// Three languages, three questions (see docs/UI-CONVENTIONS.md § Loading):
//   Skeleton     has the FETCH resolved?        → signal static
//   Generating   what SHAPE is the output?      → bar rows (this file)
//   WaitingMark  is the agent still WORKING?    → lib/waiting
//
// Components: Generating / GeneratingBars / GeneratingDots / GeneratingHelix /
// GeneratingOverlay (.svelte); the shared motif tables live here. GeneratingDots
// and GeneratingHelix are no longer used in-app — `GeneratingDots` stays because
// it is a public SDK export (sdk/index.ts), and `GeneratingHelix` stays as the
// only multi-tone loader the kit has.

export type GeneratingVariant = 'weave' | 'scan' | 'breathe' | 'helix' | 'pulse'

export const MOTIF: Record<GeneratingVariant, string> = {
  weave: 'gd-weave',
  scan: 'gd-scan',
  breathe: 'gd-breathe',
  helix: 'gd-helix',
  pulse: 'gd-pulse',
}

/** CONTEXT HELIX tones (spec §9): chart-1 / gold / chart-2. */
export const HELIX_TONES = ['text-chart-1', 'text-accent', 'text-chart-2']

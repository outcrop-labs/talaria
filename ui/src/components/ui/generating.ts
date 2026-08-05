// Generation-in-progress — Mercury motion grammar (spec §9). Loaders are
// rows of small rounded bars (3×12px, like the chip meters) on the gd-*
// motifs: weave (submitting burst), scan (agent stage / tool activity),
// breathe (ambient background monitor), helix (reasoning / generating loop),
// pulse (two-state fallback — also the reduced-motion swap, handled in
// styles.css). Calmer than a spinner, honest about indeterminate work. Use
// `lines={0}` for just the status line.
//
// Components: Generating / GeneratingBars / GeneratingDots / GeneratingHelix /
// GeneratingOverlay (.svelte); the shared motif tables live here.

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

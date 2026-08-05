// The confab guard's caveat on a flagged agent reply (annotate/strict modes).
// GuardCaveat.svelte renders these from message metadata — the findings are
// never part of the reply text, so they can't leak back into any transcript.

export interface GuardFinding {
  check: string
  severity: 'low' | 'medium' | 'high'
  confidence: number
  message: string
  snippet: string
}

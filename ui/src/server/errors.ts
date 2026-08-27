// The two shapes every server catch-block wants from an unknown thrown
// value. Ten files carried private copies of one or both (one under the
// other's name — updater.ts's `errText` was message-only), and the copies
// were exactly alike except where they weren't.
//
//   errText — the stack when there is one: for logs, where the frame trail
//             is the point.
//   errLine — the message alone: for a status line or a run note a human
//             reads inline, where a stack is noise.

export const errText = (e: unknown): string => (e instanceof Error ? (e.stack ?? e.message) : String(e))

export const errLine = (e: unknown): string => (e instanceof Error ? e.message : String(e))

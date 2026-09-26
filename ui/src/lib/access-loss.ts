/** The confirm-dialog copy for a board team move's access-loss preview
 *  (TALA-38). Pure string shaping, split out of boards.svelte.ts so the
 *  wording is unit-testable without importing the app's module graph. */

/** "Ann <ann@x>" — the name when the row has one, else the email, else the
 *  user id (an account that never finished signing in). Same precedence the
 *  member lists render by. */
export const lossName = (m: { name: string | null; email: string | null; userId: string }) =>
  m.name?.trim() || m.email || m.userId

/** The dialog body for the people who would lose sight of the board. An
 *  empty list must say so plainly — "nothing will happen" is information
 *  too, and omitting the section reads as a broken preview. */
export const accessLossMessage = (names: string[]): string => {
  if (!names.length) return 'Nobody loses access.'
  return `${names.map((n) => `• ${n}`).join('\n')}\n\nThey lose access the moment the move lands.`
}

// The turn-landing edge, extracted from ChatView.svelte so it can be tested
// without mounting the whole chat (no component-test precedent exists in
// ui/src — every test is a pure module test).
//
// TALA-33: the arm used to be a BOOLEAN, which lived across conversation
// switches — armed in one thread, it could fire (or fail to fire) in another
// after the selection changed. The arm is now the CONVERSATION ID that had a
// turn in flight, and a landing only counts for the thread that armed it.

/** The conversation id with a turn in flight, or null. */
export type TurnArm = string | null

/** One evaluation of the landing edge: the next arm, and whether the turn
 * that just landed was one this thread was watching.
 *
 * `inFlight` is (streaming || resuming); `landed` is "the last row is a
 * complete assistant reply". The rules, in order:
 *   · a conversation switch retires the old arm without firing — the new
 *     thread's rows are not this thread's landing, and a stale arm firing on
 *     load was the false onTurnComplete the boolean allowed;
 *   · a turn in flight arms (or re-arms) the CURRENT conversation;
 *   · a landing fires only for the conversation that armed it, then clears;
 *   · anything else keeps the arm as it was — the long middle of a turn,
 *     and the whole life of an old conversation that never armed. */
export function landingTransition(
  arm: TurnArm,
  convId: string | null,
  inFlight: boolean,
  landed: boolean,
): { arm: TurnArm; fire: boolean } {
  // A switch of thread (or the new-chat reset to null) clears the arm —
  // never fires. The old thread's landing is not this thread's news.
  if (arm !== null && arm !== convId) return { arm: null, fire: false }
  if (inFlight && !landed) return { arm: convId, fire: false }
  if (landed && arm !== null) return { arm: null, fire: true }
  return { arm, fire: false }
}

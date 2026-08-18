// THE DOCUMENT YOU WERE READING, remembered by Knowledge itself.
//
// See `view-memory.ts` for why this is a stored SELECTION rather than a stored
// href, and for the two rules the view has to follow when using it.
//
// Knowledge already puts its selection in the path — `/knowledge/<space>/<doc>`
// — and derives everything from it. Leaving drops that, and the view's own
// canonicalising effect then lands you on the first space, so coming back from
// a board lost the document you had open.

import { viewMemory } from './view-memory'

export interface KnowledgeSelection {
  spaceId: string
  /** null = the space overview, which is a real place to be, not "nothing". */
  docId: string | null
}

function parse(raw: unknown): KnowledgeSelection | null {
  if (!raw || typeof raw !== 'object') return null
  const v = raw as Record<string, unknown>
  if (typeof v.spaceId !== 'string' || !v.spaceId) return null
  return { spaceId: v.spaceId, docId: typeof v.docId === 'string' && v.docId ? v.docId : null }
}

const memory = viewMemory<KnowledgeSelection>('talaria:knowledge-selection', parse)

export function readKnowledgeSelection(): KnowledgeSelection | null {
  return memory.read()
}

export function writeKnowledgeSelection(sel: KnowledgeSelection): void {
  memory.write(sel)
}

/**
 * The remembered selection, if it still refers to something that exists.
 *
 * VALIDATED AGAINST THE LIVE SPACE LIST, because the alternative is landing
 * someone in a space that was deleted or unshared — a worse outcome than the
 * first-space default this replaces.
 *
 * THE DOC IS TAKEN ON TRUST, deliberately. The document list is per-space and
 * is not loaded until a space is active, so at restore time there is nothing to
 * validate against; discarding a good doc id for a list that merely has not
 * arrived yet is the same race in a new place. A doc that really is gone is the
 * editor's own not-found story, which already exists — and unlike a bad space,
 * it degrades to the space overview rather than to nowhere.
 */
export function restorableKnowledgeSelection(
  saved: KnowledgeSelection | null,
  rosters: { spaceIds: string[] },
): KnowledgeSelection | null {
  if (!saved) return null
  return rosters.spaceIds.includes(saved.spaceId) ? saved : null
}

/** Tests only: drop the remembered selection and the load latch. */
export function resetKnowledgeSelection(): void {
  memory.reset()
}

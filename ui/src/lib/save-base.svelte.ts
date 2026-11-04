// NO SILENT OVERWRITE — the editor half of the save routes' base check
// (`overwrites_unseen` in talaria-routes-knowledge: /api/kb/docs/{id} and
// /api/artifacts/{id}).
//
// A document editor holds a buffer seeded from one version of the document
// and saves the whole body back. An agent's `edit_kb_doc` / `update_document`
// landing while that buffer is open used to be wiped by the person's next
// save, with nothing on screen to say so. The editor now names the version
// its buffer started from (`baseUpdatedAt`) and the route refuses a stale
// one with the document as it now is. This module decides what that refusal
// means: a move that left the body alone (an icon, a re-file, a promote) is
// retried on the fresh version without a word; a body someone else changed
// becomes `conflict`, the one inline line the editor shows — at save time,
// never on open.

export interface Versioned {
  updatedAt: string
  body: string
}

/** The version an editing surface's buffer started from. */
export interface SaveBase {
  updatedAt: string
  body: string
}

/** A guarded PUT's two answers: saved, or refused with the current row. */
export type SaveAnswer<T> = { saved: T } | { conflict: T }

export const baseOf = (x: Versioned): SaveBase => ({ updatedAt: x.updatedAt, body: x.body })

/** A refused save, decided. The row moved, but if its body is still the one
 *  the buffer started from, only metadata moved and the save is safe on the
 *  fresh version. Otherwise someone else's text is there. */
export function conflictVerdict(base: SaveBase, current: Versioned): 'retry' | 'conflict' {
  return current.body === base.body ? 'retry' : 'conflict'
}

/** What the base becomes after a save landed. A body save makes the saved row
 *  the base. A save without a body (icon, sharing, routing) only advances the
 *  version while the body is still the base's: if someone else's body is in
 *  the row now, the base stays where the buffer is, so the next body save is
 *  still checked against it. */
export function baseAfterSave(base: SaveBase | null, saved: Versioned, sentBody: boolean): SaveBase | null {
  if (sentBody) return baseOf(saved)
  if (base && saved.body === base.body) return baseOf(saved)
  return base
}

/** One editor's save door. Saves run ONE AT A TIME, in call order: two
 *  autosaves in flight at once would race (the older body landing last), and
 *  the second would carry a base the first had already moved. `save` resolves
 *  the saved row, or null when the save was refused as a conflict; any other
 *  failure rejects, as before. `guard: true` is for the editing surface's own
 *  buffer; an explicit replace (a restore from version history, "keep mine")
 *  saves unguarded. */
export function guardedSaver<T extends Versioned, P extends { body?: string }>(
  put: (patch: P & { baseUpdatedAt?: string }) => Promise<SaveAnswer<T>>,
) {
  let base = $state<SaveBase | null>(null)
  let conflict = $state(false)
  let chain: Promise<unknown> = Promise.resolve()

  const run = async (patch: P, guard: boolean): Promise<T | null> => {
    const sentBody = patch.body !== undefined
    const checked = guard && sentBody && base !== null
    // Already refused: the buffer is still stale, so the answer would be too.
    if (checked && conflict) return null
    let answer = await put(checked ? { ...patch, baseUpdatedAt: base!.updatedAt } : patch)
    if ('conflict' in answer && base && conflictVerdict(base, answer.conflict) === 'retry') {
      base = baseOf(answer.conflict)
      answer = await put({ ...patch, baseUpdatedAt: base.updatedAt })
    }
    if ('conflict' in answer) {
      conflict = true
      return null
    }
    base = baseAfterSave(base, answer.saved, sentBody)
    if (sentBody) conflict = false
    return answer.saved
  }

  return {
    get base() {
      return base
    },
    /** A body save was refused: someone else's text is in the document. */
    get conflict() {
      return conflict
    },
    /** The editing surface was (re)seeded from `x` — its buffer is that now. */
    rebase(x: Versioned | null | undefined) {
      base = x ? baseOf(x) : null
      conflict = false
    },
    save(patch: P, opts: { guard?: boolean } = {}): Promise<T | null> {
      const p = chain.then(() => run(patch, opts.guard ?? false))
      chain = p.catch(() => {})
      return p
    },
  }
}

// NO SILENT OVERWRITE — the editor half of the save routes' version check
// (`save_doc_content_if_unchanged` / `save_artifact_content_if_unchanged`,
// behind PUT /api/kb/docs/{id} and PUT /api/artifacts/{id}).
//
// A document editor holds a buffer seeded from one version of the document
// and saves the whole body back. An agent's `edit_kb_doc` / `update_document`
// landing while that buffer is open used to be wiped by the person's next
// save, with nothing on screen to say so. The editor now names the version
// its buffer started from (`baseUpdatedAt`) and the route refuses a stale
// one with the document as it now is. This module decides what that refusal
// means: a move that left the body (and the title) alone — an icon, a
// re-file, a promote — is retried on the fresh version without a word; a
// body someone else changed becomes `conflict`, the one inline line the
// editor shows, at save time and never on open. What the person typed after
// that is `held` here, and lib/held-edit keeps it on the device.

export interface Versioned {
  updatedAt: string
  body: string
  title?: string
}

/** The version an editing surface's buffer started from. */
export interface SaveBase {
  updatedAt: string
  body: string
  title?: string
}

/** A guarded PUT's two answers: saved, or refused with the current row. */
export type SaveAnswer<T> = { saved: T } | { conflict: T }

export const baseOf = (x: Versioned): SaveBase =>
  x.title === undefined ? { updatedAt: x.updatedAt, body: x.body } : { updatedAt: x.updatedAt, body: x.body, title: x.title }

/** A refused save, decided. The row moved; the save is safe on the fresh
 *  version only if nothing it would replace changed: the body is still the
 *  one the buffer started from, and — when the save renames — the title is
 *  too (two renames are a conflict, not a retry). Anything else is someone
 *  else's work in the document. */
export function conflictVerdict(base: SaveBase, current: Versioned, patch: { title?: string } = {}): 'retry' | 'conflict' {
  if (current.body !== base.body) return 'conflict'
  if (patch.title !== undefined && current.title !== base.title) return 'conflict'
  return 'retry'
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
 *  saves unguarded. A guarded save sends its title only when it differs from
 *  the base's, so an unchanged title can never undo someone else's rename.
 *
 *  `rebase` starts a new generation: a save still in flight when the surface
 *  was reseeded lands on the server, but it no longer moves this base or
 *  raises the line — the buffer it came from is gone, and letting it reset
 *  the base would let the reseeded buffer pass the check over its own lost
 *  words. */
export function guardedSaver<T extends Versioned, P extends { body?: string; title?: string }>(
  put: (patch: P & { baseUpdatedAt?: string }) => Promise<SaveAnswer<T>>,
) {
  let base = $state<SaveBase | null>(null)
  let conflict = $state(false)
  let held = $state<P | null>(null)
  let busy = $state(0)
  let gen = 0
  let chain: Promise<unknown> = Promise.resolve()

  const run = async (patch: P, guard: boolean): Promise<T | null> => {
    const sentBody = patch.body !== undefined
    const checked = guard && sentBody && base !== null
    // Already refused: the buffer is still stale, so the answer would be too.
    // Keep what was typed — it is the person's, and lib/held-edit stores it.
    if (checked && conflict) {
      held = patch
      return null
    }
    const mine = gen
    const send: P & { baseUpdatedAt?: string } = { ...patch }
    if (checked) {
      send.baseUpdatedAt = base!.updatedAt
      if (send.title !== undefined && send.title === base!.title) delete send.title
    }
    let answer = await put(send)
    if (mine !== gen) return 'saved' in answer ? answer.saved : null
    if ('conflict' in answer && base && conflictVerdict(base, answer.conflict, send) === 'retry') {
      base = baseOf(answer.conflict)
      answer = await put({ ...send, baseUpdatedAt: base.updatedAt })
      if (mine !== gen) return 'saved' in answer ? answer.saved : null
    }
    if ('conflict' in answer) {
      conflict = true
      held = patch
      return null
    }
    base = baseAfterSave(base, answer.saved, sentBody)
    if (sentBody) {
      conflict = false
      held = null
    }
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
    /** The last buffer a refused save carried — the person's unsaved words. */
    get held() {
      return held
    },
    /** A save is in flight or queued. */
    get busy() {
      return busy > 0
    },
    /** The editing surface was (re)seeded from `x` — its buffer is that now. */
    rebase(x: Versioned | null | undefined) {
      gen += 1
      base = x ? baseOf(x) : null
      conflict = false
      held = null
    },
    /** Resolves once every save asked for so far has answered. */
    idle(): Promise<void> {
      return chain.then(() => undefined)
    },
    save(patch: P, opts: { guard?: boolean } = {}): Promise<T | null> {
      busy += 1
      const p = chain.then(() => run(patch, opts.guard ?? false)).finally(() => (busy -= 1))
      chain = p.catch(() => {})
      return p
    },
  }
}

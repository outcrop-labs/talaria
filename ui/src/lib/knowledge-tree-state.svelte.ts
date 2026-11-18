// THE SIDEBAR TREE'S EXPANSION STATE, remembered across navigation.
//
// See `view-memory.ts` for the house pattern this follows, and
// `knowledge-selection.ts` for its sibling: the selection memory remembers
// WHERE YOU WERE, this one remembers WHICH BRANCHES WERE OPEN.
//
// WHY THIS IS A STORE AND NOT ROW STATE: every KbDocRow held its own
// `expanded = $state(true)` (TALA-92), so the tree's shape lived and died
// with each row's mount — switch spaces, leave to a board and come back,
// and every branch you had opened was closed again. Expansion is a fact
// about the TREE, not about a row, so it lives here, one singleton for the
// whole knowledgebase, persisted through the same session-scoped memory.
//
// THE DEFAULT IS COLLAPSED: absent key = collapsed, for docs and spaces
// both. That is what makes the tolerant parse and the absent-is-default
// read one rule: a garbage or stale entry — a deleted doc, a doc moved to
// another space, a shape an older release wrote — can only ever mean "not
// open", never "open", and nothing has to be validated against any roster
// to say so.
//
// PERSISTED ON EVERY MUTATION, READ ONCE ON ARRIVAL: viewMemory reads
// sessionStorage once per tab and mirrors it in memory afterwards (private
// mode included). The singleton hydrates from it lazily on first use —
// not at module load — so a test (or anything else) can install its
// sessionStorage before the first read burns viewMemory's load latch.

import { viewMemory } from './view-memory'

export interface KbTreeState {
  /** `docs[id]` = whether that doc node's children are shown. */
  docs: Record<string, boolean>
  /** `spaces[id]` = whether that space's section in the sidebar is shown. */
  spaces: Record<string, boolean>
}

function parse(raw: unknown): KbTreeState | null {
  if (!raw || typeof raw !== 'object') return null
  const docs: Record<string, boolean> = {}
  const spaces: Record<string, boolean> = {}
  const v = raw as Record<string, unknown>
  if (v.docs && typeof v.docs === 'object') {
    for (const [k, val] of Object.entries(v.docs)) {
      if (typeof val === 'boolean') docs[k] = val
    }
  }
  if (v.spaces && typeof v.spaces === 'object') {
    for (const [k, val] of Object.entries(v.spaces)) {
      if (typeof val === 'boolean') spaces[k] = val
    }
  }
  return { docs, spaces }
}

const memory = viewMemory<KbTreeState>('talaria:knowledge-tree-state', parse)

// The reactive half. `$state` on the whole object (not SvelteMap) so
// property writes are fine-grained and reads stay reactive.
const store = $state<KbTreeState>({ docs: {}, spaces: {} })
let hydrated = false

/** Pull the persisted shape into the reactive store, once per session.
 *
 * WHY THIS IS PUBLIC ON THE OBJECT: the first read of a stored shape
 * writes $state right here, and Svelte forbids writing state inside
 * $derived(...) or a template expression (state_unsafe_mutation) — the
 * live crash we hit on /knowledge with a stored key, because KbDocRow
 * reads `docExpanded` in a $derived and Knowledge.svelte reads
 * `sectionExpanded` in an {#if}. Knowledge.svelte calls this once in its
 * instance script — a plain statement, before anything renders — so
 * every later derived/template read is a pure read. */
function hydrate(): void {
  if (hydrated) return
  hydrated = true
  const saved = memory.read()
  if (saved) {
    store.docs = saved.docs
    store.spaces = saved.spaces
  }
}

/** Read-modify-write: every mutation persists the FULL shape, because the
 * viewMemory mirror is the tab's whole memory and a partial write would
 * be a shape the parser does not produce. */
function persist(): void {
  memory.write({ docs: { ...store.docs }, spaces: { ...store.spaces } })
}

export const kbTreeState = {
  /** Pull the persisted shape into the store now, idempotently. Call
   *  once from a non-reactive context (Knowledge.svelte's instance
   *  script) BEFORE any derived/template read, so the $state write above
   *  never happens inside $derived evaluation or a template expression. */
  hydrate,
  /** Whether a doc node's children are shown. Absent = collapsed. */
  docExpanded(id: string): boolean {
    hydrate()
    return store.docs[id] ?? false
  },
  /** Open or close a doc node, and remember it. Absent = collapsed, so
   *  closing an already-collapsed node is a no-op that does not write. */
  toggleDoc(id: string, expanded: boolean): void {
    hydrate()
    if ((store.docs[id] ?? false) === expanded) return
    store.docs[id] = expanded
    persist()
  },
  /** Whether a space's section is shown. Absent = collapsed. */
  sectionExpanded(id: string): boolean {
    hydrate()
    return store.spaces[id] ?? false
  },
  /** Open or close a space's section, and remember it. Absent = collapsed,
   *  so closing an already-collapsed section does not write either. */
  toggleSection(id: string, expanded: boolean): void {
    hydrate()
    if ((store.spaces[id] ?? false) === expanded) return
    store.spaces[id] = expanded
    persist()
  },
  /** Open a whole path — every doc on the chain plus the space section —
   * WITHOUT closing anything. Used by restore, nested create, and drop
   * 'inside': each of those puts a node somewhere it must be SEEN, and
   * seeing it means every ancestor's branch is open. Never closes: an
   * openPath that closed other branches would be a navigation that
   * rearranged the tree behind your back. */
  openPath(docIds: string[], spaceId: string): void {
    hydrate()
    for (const id of docIds) store.docs[id] = true
    store.spaces[spaceId] = true
    persist()
  },
}

/** Tests only: drop the persisted memory and the reactive store, and
 * re-arm the hydrate latch so the next read starts from scratch. */
export function resetKbTreeState(): void {
  memory.reset()
  store.docs = {}
  store.spaces = {}
  hydrated = false
}
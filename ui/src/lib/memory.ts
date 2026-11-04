// Memory client — the two documents an agent keeps: the record it curates
// about its work, and the profile it keeps about its person. Both are the
// agent's own files, editable by hand.
//
// Two surfaces read this (the personal assistant's panel and the fleet
// agent's) and each had its own copy of the calls, with the usual drift:
// different query keys for the same document (`['assistant-memory', id]` vs
// `['memory', id]`), only one sending credentials, and — the one that mattered
// — BOTH ignoring whether the save succeeded. One did `await fetch(...)` and
// never looked at the response; the other checked the body for `{ error }` but
// not the status, so a 403 with an empty body counted as a write. Either way a
// refused save reported success and the edit was gone.
//
// `getJson` forces same-origin and throws on any non-2xx, so going through
// here is what makes a refused write visible.
import { createQuery } from '@tanstack/svelte-query'
import { getJson } from '@/lib/fetch-json'

/** WHICH DOCUMENT. Hermes keeps two files side by side in the same place
 *  (`/opt/data/memories/`): `MEMORY.md`, the record the agent curates about
 *  its work, and `USER.md`, the profile it keeps about its person. They are
 *  read and written the same way and they are NOT interchangeable, which is
 *  why this is a named union rather than a loose string — a value that
 *  resolved to the other file would overwrite the wrong document, and every
 *  write here is a whole-file write. */
export type MemoryFile = 'memory' | 'profile'

export interface MemoryDoc {
  content: string
  /** The container it was read from. Present for managed fleet agents. */
  container: string
}

/** One key per DOCUMENT, not per agent: the entries view switches files in
 *  place, and a shared key would hand the profile the memory's cached content
 *  for a frame — long enough for a save to write one back over the other. */
export const memoryKey = (id: string, file: MemoryFile = 'memory') => ['memory', id, file] as const

const url = (id: string, file: MemoryFile) => `/api/memory/${id}?file=${file}`

export function useMemory(id: () => string, file: () => MemoryFile = () => 'memory') {
  return createQuery(() => {
    // READ ONCE, HERE: the key and the request must name the same document.
    // Reading the getters inside `queryFn` instead would let a switch that
    // moves while a read is in flight resolve one file's content under the
    // other file's key — and the next save writes what is on screen.
    const [agent, doc] = [id(), file()]
    return {
      queryKey: memoryKey(agent, doc),
      // GET is 200 `{ content, container }`, or 4xx `{ error }` when the
      // container cannot be reached. A missing `content` on a 2xx is a genuinely
      // empty memory; a non-2xx is a failure and must not be flattened into one —
      // that is how "Nothing remembered yet" got printed about a document nobody
      // managed to read.
      queryFn: async (): Promise<MemoryDoc> => {
        const j = await getJson<{ content?: string; container?: string }>(url(agent, doc))
        return { content: j.content ?? '', container: j.container ?? '' }
      },
      // The agent is either reachable or it is not; retrying a 403 just delays
      // the sentence that explains it.
      retry: false,
      // Every change reaches the component, not only changes to what it has
      // read so far. MemoryEditor reads `isError` on mount but `data` only in
      // the Whole file view, and svelte-query tells a component about a
      // change only when it touches a prop already read (anyLoading in
      // components/ui/query-state.ts says why). So a read that landed while
      // Entries was on screen was never passed on, and Whole file sat on its
      // skeleton for good. One document, no polling: notifying on every
      // change costs nothing here.
      notifyOnChangeProps: 'all',
    }
  })
}

export async function saveMemory(id: string, content: string, file: MemoryFile = 'memory'): Promise<void> {
  await getJson<{ ok?: true }>(url(id, file), {
    method: 'PUT',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ content }),
  })
}

/** Append one dated line to the document.
 *
 *  Takes the CURRENT content rather than re-reading it: the caller already has
 *  the loaded document (here, the editor's live text), and re-fetching here
 *  would open a window where an agent write lands between the read and the
 *  append and is overwritten. The caller must not call this before its read
 *  has landed, for the same reason — appending to `''` would clobber the
 *  whole file.
 *
 *  Returns the content it wrote, so the caller can stage that exact text in
 *  its editor: a fact just appended can never be clobbered by a later Save
 *  from an editor still showing the pre-append file.
 */
export async function appendMemoryNote(
  id: string,
  current: string,
  note: string,
  today: string,
  file: MemoryFile = 'memory',
): Promise<string> {
  const base = current.replace(/\s+$/, '')
  const content = `${base ? `${base}\n` : ''}- ${note} _(added by hand, ${today})_\n`
  await saveMemory(id, content, file)
  return content
}

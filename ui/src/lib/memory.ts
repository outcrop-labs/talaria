// Memory client — one MEMORY.md per agent, curated by the agent and editable
// by hand.
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

export interface MemoryDoc {
  content: string
  /** The container it was read from. Present for managed fleet agents. */
  container: string
}

export const memoryKey = (id: string) => ['memory', id] as const

export function useMemory(id: () => string) {
  return createQuery(() => ({
    queryKey: memoryKey(id()),
    // GET is 200 `{ content, container }`, or 4xx `{ error }` when the
    // container cannot be reached. A missing `content` on a 2xx is a genuinely
    // empty memory; a non-2xx is a failure and must not be flattened into one —
    // that is how "Nothing remembered yet" got printed about a document nobody
    // managed to read.
    queryFn: async (): Promise<MemoryDoc> => {
      const j = await getJson<{ content?: string; container?: string }>(`/api/memory/${id()}`)
      return { content: j.content ?? '', container: j.container ?? '' }
    },
    // The agent is either reachable or it is not; retrying a 403 just delays
    // the sentence that explains it.
    retry: false,
  }))
}

export async function saveMemory(id: string, content: string): Promise<void> {
  await getJson<{ ok?: true }>(`/api/memory/${id}`, {
    method: 'PUT',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ content }),
  })
}

/** Append one dated line to the document.
 *
 *  Takes the CURRENT content rather than re-reading it: the caller already has
 *  the loaded document, and re-fetching here would open a window where an
 *  agent write lands between the read and the append and is overwritten. The
 *  caller must not call this before its read has landed, for the same reason —
 *  appending to `''` would clobber the whole file.
 */
export async function appendMemoryNote(id: string, current: string, note: string, today: string): Promise<void> {
  const base = current.replace(/\s+$/, '')
  await saveMemory(id, `${base ? `${base}\n` : ''}- ${note} _(added by hand, ${today})_\n`)
}

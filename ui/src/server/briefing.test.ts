// The one thing that got HARDER when the briefing chat's tee transport was
// deleted, and the reason it has a test of its own.
//
// The tee relayed the persona's own SSE frames byte for byte, so `briefing.ts`
// never had to know the wire format. `runHarnessStreamed` hands over CONTENT
// DELTAS instead — which is what makes the guard pass possible on this surface —
// so the file now re-frames them, and a mismatch between what it writes and what
// `AssistantBriefing.svelte` parses is silent: no error, no log, just a panel
// that streams nothing while the owner watches an empty bubble.
//
// So this feeds the frames through the REAL parser the panel uses rather than
// asserting on a string shape, because "the string looks right" is exactly the
// belief that would let a `data:` prefix or a frame separator go missing.
import { describe, expect, it } from 'vitest'
import { parseAgentStream } from '@/lib/sse-parse'
import { contentFrame } from '@/server/briefing'

const streamOf = (chunks: string[]): ReadableStream<Uint8Array> => {
  const encoder = new TextEncoder()
  return new ReadableStream<Uint8Array>({
    start(controller) {
      for (const c of chunks) controller.enqueue(encoder.encode(c))
      controller.close()
    },
  })
}

const contentOf = async (chunks: string[]): Promise<string> => {
  let text = ''
  for await (const ev of parseAgentStream(streamOf(chunks))) if (ev.type === 'content') text += ev.text
  return text
}

describe('the briefing chat wire', () => {
  it('round-trips the deltas the panel renders', async () => {
    const deltas = ['"Ledger ', 'migration" is ', 'the blocked one.']
    expect(await contentOf(deltas.map(contentFrame))).toBe(deltas.join(''))
  })

  it('survives a delta arriving split across two network chunks', async () => {
    // `parseAgentStream` buffers on the frame separator, so a frame torn in half
    // by the transport must still parse. This is the failure the tee could never
    // have had — it forwarded whole frames the persona had already built.
    const frame = contentFrame('half here, half there')
    const cut = Math.floor(frame.length / 2)
    expect(await contentOf([frame.slice(0, cut), frame.slice(cut)])).toBe('half here, half there')
  })

  it('escapes a reply that contains the things a naive frame would break on', async () => {
    // A briefing answer quotes ticket titles verbatim, and a title with a
    // newline, a quote or a backslash in it would tear the frame apart if this
    // were string concatenation instead of JSON.
    const nasty = 'the ticket is called "a\\b\nc" — mind the newline'
    expect(await contentOf([contentFrame(nasty)])).toBe(nasty)
  })
})

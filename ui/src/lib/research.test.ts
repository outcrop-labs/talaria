// The research ask's pure half: the dial presets a mode stands for, the axis
// catalog the pickers render, and what `startResearch` actually puts on the
// wire — including the two sentences a 409 can mean.
import { afterEach, describe, expect, it, vi } from 'vitest'
import {
  DIAL_AXES,
  MODE_DIALS,
  MODE_META,
  dialsForMode,
  duplicateReasonInThread,
  duplicateRunMessage,
  startResearch,
  type ResearchMode,
} from '@/lib/research'

const jsonRes = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json' } })

/** The one run the happy path hands back; only the fields anyone reads. */
const run = { id: 'r-1', conversationId: 'c-1' }

/** The body the last fetch sent, parsed. */
const sentBody = (fetchMock: ReturnType<typeof vi.fn>): Record<string, unknown> =>
  JSON.parse(String((fetchMock.mock.calls[0]?.[1] as RequestInit).body))

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('MODE_DIALS', () => {
  it('holds the three presets the server keeps', () => {
    // Dials::from_mode in talaria-research-def. A drift here would send a
    // body that disagrees with the mode it names.
    expect(MODE_DIALS.recon).toEqual({ effort: 1, breadth: 1, sources: 1 })
    expect(MODE_DIALS.brief).toEqual({ effort: 1, breadth: 2, sources: 2 })
    expect(MODE_DIALS.expedition).toEqual({ effort: 3, breadth: 3, sources: 3 })
  })

  it('covers every mode the picker offers', () => {
    for (const mode of Object.keys(MODE_META) as ResearchMode[]) {
      expect(MODE_DIALS[mode]).toBeDefined()
    }
  })

  it('every axis of every preset is in band', () => {
    for (const dials of Object.values(MODE_DIALS)) {
      for (const v of [dials.effort, dials.breadth, dials.sources]) {
        expect(v).toBeGreaterThanOrEqual(1)
        expect(v).toBeLessThanOrEqual(3)
      }
    }
  })
})

describe('dialsForMode', () => {
  it('reads the preset', () => {
    expect(dialsForMode('brief')).toEqual({ effort: 1, breadth: 2, sources: 2 })
  })

  it('hands out a COPY — a moved dial must not edit the preset', () => {
    const mine = dialsForMode('recon')
    mine.effort = 3
    expect(MODE_DIALS.recon.effort).toBe(1)
    expect(dialsForMode('recon')).toEqual({ effort: 1, breadth: 1, sources: 1 })
  })
})

describe('DIAL_AXES', () => {
  it('names the three axes, in dial order, each with three rungs', () => {
    expect(DIAL_AXES.map((a) => a.key)).toEqual(['effort', 'breadth', 'sources'])
    expect(DIAL_AXES.map((a) => a.label)).toEqual(['Effort', 'Breadth', 'Sources'])
    for (const axis of DIAL_AXES) {
      expect(axis.steps).toHaveLength(3)
      expect(axis.note.trim()).not.toBe('')
      // UI-CONVENTIONS: no ellipsis in UI copy.
      for (const copy of [axis.label, axis.note, ...axis.steps]) expect(copy).not.toMatch(/…|\.\.\./)
    }
  })

  it("the rungs read out the server's own ladders", () => {
    // Dials::rounds (1/2/3), queries (1/3/4), max_sources (6/12/24).
    const steps = Object.fromEntries(DIAL_AXES.map((a) => [a.key, a.steps]))
    expect(steps.effort).toEqual(['1 round', '2 rounds', '3 rounds'])
    expect(steps.breadth).toEqual(['1 query', '3 queries', '4 queries'])
    expect(steps.sources).toEqual(['6 sources', '12 sources', '24 sources'])
  })
})

describe('duplicateReasonInThread', () => {
  it('follows the reason when the route gives one, and the ask when it does not', () => {
    expect(duplicateReasonInThread('thread', false)).toBe(true)
    expect(duplicateReasonInThread('question', true)).toBe(false)
    expect(duplicateReasonInThread(undefined, true)).toBe(true)
    expect(duplicateReasonInThread(undefined, false)).toBe(false)
    expect(duplicateReasonInThread('something-new', true)).toBe(true)
  })
})

describe('duplicateRunMessage', () => {
  it('names the SESSION when the ask was pointed at one', () => {
    expect(duplicateRunMessage(true)).toBe('this session already has a run going')
  })

  it('names the question when it was not', () => {
    expect(duplicateRunMessage(false)).toBe('that question is already being researched')
  })
})

describe('startResearch', () => {
  it('sends the dials at the top level, beside the mode', async () => {
    const f = vi.fn(async () => jsonRes({ run }))
    vi.stubGlobal('fetch', f)
    await startResearch('what changed?', 'expedition', 'claude-opus-5', { effort: 3, breadth: 1, sources: 2 })
    expect(sentBody(f)).toEqual({
      question: 'what changed?',
      mode: 'expedition',
      agentModel: 'claude-opus-5',
      effort: 3,
      breadth: 1,
      sources: 2,
    })
  })

  it('sends no dial keys at all when the caller moved none', async () => {
    // ABSENT IS NOT A DIAL SETTING — the route reads "no dials" and the run
    // keeps reading as its mode rather than as three numbers.
    const f = vi.fn(async () => jsonRes({ run }))
    vi.stubGlobal('fetch', f)
    await startResearch('q', 'recon', 'm')
    expect(Object.keys(sentBody(f)).sort()).toEqual(['agentModel', 'mode', 'question'])
  })

  it('rides the conversation id in the body when one was given', async () => {
    const f = vi.fn(async () => jsonRes({ run }))
    vi.stubGlobal('fetch', f)
    await startResearch('q', 'brief', 'm', dialsForMode('brief'), 'conv-9')
    expect(sentBody(f).conversationId).toBe('conv-9')
  })

  it('omits the key entirely when no conversation was named', async () => {
    const f = vi.fn(async () => jsonRes({ run }))
    vi.stubGlobal('fetch', f)
    await startResearch('q', 'brief', 'm')
    expect('conversationId' in sentBody(f)).toBe(false)
  })

  it('a 409 the route calls a thread conflict reads as the session already running one', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => jsonRes({ run: null, duplicateOf: 'r-7', reason: 'thread' }, 409)))
    await expect(startResearch('q', 'brief', 'm', undefined, 'conv-9')).rejects.toThrow(
      'this session already has a run going',
    )
  })

  it('a 409 the route calls a question conflict reads as the duplicate question even from a thread', async () => {
    // The in-thread ask collided with somebody else's identical question: the
    // sentence must not name a run that is not in this session.
    vi.stubGlobal('fetch', vi.fn(async () => jsonRes({ run: null, duplicateOf: 'r-7', reason: 'question' }, 409)))
    await expect(startResearch('q', 'brief', 'm', undefined, 'conv-9')).rejects.toThrow(
      'that question is already being researched',
    )
  })

  it('a 409 with no reason (an older api) falls back to whether the ask named a thread', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => jsonRes({ run: null, duplicateOf: 'r-7' }, 409)))
    await expect(startResearch('q', 'brief', 'm', undefined, 'conv-9')).rejects.toThrow(
      'this session already has a run going',
    )
    vi.stubGlobal('fetch', vi.fn(async () => jsonRes({ run: null, duplicateOf: 'r-7' }, 409)))
    await expect(startResearch('q', 'brief', 'm')).rejects.toThrow('that question is already being researched')
  })

  it('a 2xx with no run still throws, with the server sentence', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => jsonRes({ run: null, error: 'the researcher is offline' })))
    await expect(startResearch('q', 'brief', 'm')).rejects.toThrow('the researcher is offline')
  })
})

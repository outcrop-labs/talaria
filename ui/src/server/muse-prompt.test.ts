// THE PROMPT MUST NOT CONTRADICT ITSELF, and it did — in the one place a
// contradiction is invisible to everybody who wrote either half.
//
// `SYSTEM.personality` asks for "Plain prose ... no headings". A separate block,
// appended to every kind in `ORG_KINDS`, told the same model to name the
// business in `## Who you are`. Neither author was wrong about their own half;
// the model received both and resolved it the only way it could.
//
// WHAT IT COST. Four models — gemma-4-31b, gemma-4-26b, haiku-4.5 and
// muse-glimmer — all opened with the identical string `## Who you are`, and the
// fitness suite recorded four model failures across two fixtures for following
// the more specific of two instructions we sent. Four different models producing
// the same string is never four models being wrong.
import { describe, expect, it, vi } from 'vitest'

vi.mock('@/server/org', () => ({
  orgProfile: async () => ({ name: 'Outcrop Labs' }),
  orgLine: () => 'Outcrop Labs — builds Talaria, the open-source agentic operations platform',
}))
vi.mock('@/server/harness/model', () => ({ resolveHarnessModel: async () => null }))

const { buildMuseMessages } = await import('@/server/muse')

const systemFor = async (kind: 'personality' | 'soul' | 'agent'): Promise<string> => {
  const msgs = await buildMuseMessages({ kind, instruction: 'go' } as Parameters<typeof buildMuseMessages>[0])
  return msgs.find((m) => m.role === 'system')?.content ?? ''
}

describe('the org anchor', () => {
  it('does NOT name a heading in the one prompt that forbids headings', async () => {
    const personality = await systemFor('personality')
    // The org context still arrives — it is what keeps an assistant anchored to
    // the business rather than to a model vendor. Only the heading is gone.
    expect(personality).toContain('Outcrop Labs')
    expect(personality).toContain('never presents itself as belonging to an underlying platform')
    expect(personality).not.toContain('## Who you are')
    // And the instruction it was contradicting is still there, unweakened.
    expect(personality).toContain('no headings')
  })

  it('still names the heading where the document actually has one', async () => {
    // A SOUL.md really does have a `## Who you are` section, and telling the
    // model to anchor the business there is right. The fix was to stop saying it
    // to a prompt with no headings at all — not to stop saying it.
    for (const kind of ['soul', 'agent'] as const) {
      expect(await systemFor(kind), kind).toContain('## Who you are')
    }
  })

  it('keeps the anchor a single sentence in every kind that gets one', async () => {
    // Cheap shape guard: the two branches are assembled by string concatenation,
    // and a missing separator is the kind of thing that reads fine in a diff.
    for (const kind of ['personality', 'soul', 'agent'] as const) {
      const s = await systemFor(kind)
      expect(s, kind).toContain("The agent is a member of this business's team")
      expect(s, kind).not.toContain('team —;')
    }
  })
})

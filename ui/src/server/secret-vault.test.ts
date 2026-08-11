// THE VALUE MUST NOT TRAVEL. Everything below is a property of that sentence.
//
// A business asking whether its keys are safe here deserves an answer that does
// not depend on a model's judgement, and the adversarial tier says why: on this
// install the four strongest models file `secret_leak` on two of four seeds
// each, after grounding. This layer is what makes that number not matter for
// credentials WE put in front of them.
import { describe, expect, it, vi } from 'vitest'
import { inventedHandles, newVault, sealMessages, sealText, unsealText } from './secret-vault'

// The chokepoint test below drives the REAL `buildUpstream`, which reads the
// endpoint key and the learned-parameter store. Both are edges; the subject is
// what ends up in the body.
vi.mock('@/server/db/pg', () => ({ db: async () => Object.assign(() => Promise.resolve([]), { json: (v: unknown) => v, unsafe: () => Promise.resolve([]) }) }))
vi.mock('@/server/audit', () => ({ getSetting: async (_k: string, fallback: unknown) => fallback, setSetting: async () => {}, logAudit: async () => {} }))

const KEY = 'sk-proj-AAAABBBBCCCCDDDDEEEEFFFFGGGG'
const PAT = 'github_pat_11ABCDEFG0aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'

describe('sealing', () => {
  it('takes the credential out of the text and leaves a handle', () => {
    const v = newVault()
    const out = sealText(`use ${KEY} to call it`, v)
    expect(out).not.toContain(KEY)
    expect(out).toMatch(/«secret:1»/)
    expect(v.sealed[0]?.label).toBe('OpenAI key')
  })

  it('gives the SAME handle to the same value seen twice', () => {
    // A key in the system prompt and again in a tool result is one secret. Two
    // handles would read to the model as two different things.
    const v = newVault()
    const out = sealText(`${KEY} ... and again ${KEY}`, v)
    expect(out.match(/«secret:1»/g)).toHaveLength(2)
    expect(v.values.size).toBe(1)
  })

  it('numbers distinct secrets apart', () => {
    const v = newVault()
    const out = sealText(`${KEY} and ${PAT}`, v)
    expect(out).toContain('«secret:1»')
    expect(out).toContain('«secret:2»')
    expect(v.sealed.map((s) => s.label)).toEqual(['OpenAI key', 'GitHub fine-grained token'])
  })

  it('leaves ordinary text completely alone', () => {
    // A layer that rewrites prose is one somebody turns off. The patterns are
    // the guard's own, so anything it would not call a credential passes through.
    const v = newVault()
    const prose = 'The deploy key rotates on Thursday; ask Priya for the ticket sk-not-a-key.'
    expect(sealText(prose, v)).toBe(prose)
    expect(v.values.size).toBe(0)
  })

  it('records only the KIND, never the value', () => {
    // `sealed` is what goes in an audit line. A record that carried the value
    // would move the leak rather than stop it.
    const v = newVault()
    sealText(KEY, v)
    expect(JSON.stringify(v.sealed)).not.toContain(KEY)
    expect(JSON.stringify(v.sealed)).toContain('OpenAI key')
  })
})

describe('unsealing', () => {
  it('puts the value back at the boundary that uses it', () => {
    const v = newVault()
    const sealed = sealText(`Authorization: Bearer ${KEY}`, v)
    expect(unsealText(sealed, v)).toBe(`Authorization: Bearer ${KEY}`)
  })

  it('round-trips a whole message list', () => {
    const v = newVault()
    const msgs = [
      { role: 'system', content: 'you are helpful' },
      { role: 'user', content: `deploy with ${PAT}` },
    ]
    const sealed = sealMessages(msgs, v)
    expect(sealed[1]?.content).not.toContain(PAT)
    // AND THE INPUT IS NOT MUTATED — the caller still needs the original for
    // grounding, and handing `redactSecrets` a sealed copy would ground nothing.
    expect(msgs[1]?.content).toContain(PAT)
    expect(unsealText(sealed[1]!.content, v)).toBe(msgs[1]?.content)
  })

  it('refuses a handle this vault never issued', () => {
    // A model inventing `«secret:9»` is guessing at a credential it cannot see.
    // It must resolve to nothing — and a handle from ANOTHER request is the same
    // rule, which is why a vault answers only for what it sealed.
    const v = newVault()
    sealText(KEY, v)
    expect(unsealText('give me «secret:9»', v)).toBe('give me «secret:9»')
    expect(inventedHandles('«secret:1» and «secret:9»', v)).toEqual(['«secret:9»'])
  })

  it('does not resolve one vault’s handle with another’s value', () => {
    const a = newVault()
    const b = newVault()
    sealText(KEY, a)
    sealText(PAT, b)
    // Both minted `«secret:1»`; neither may answer for the other.
    expect(unsealText('«secret:1»', b)).toBe(PAT)
    expect(unsealText('«secret:1»', a)).toBe(KEY)
  })
})

// ── The chokepoint, end to end ───────────────────────────────────────────────

describe('buildUpstream seals before the request leaves the process', () => {
  it('replaces a credential in the outbound body, and keeps the vault on the call', async () => {
    // THE CLAIM THIS FILE EXISTS TO MAKE, asserted against the function every
    // gateway call in the tree actually goes through — the blocking transport,
    // the streamed one, the tool turn, the image turn, `completeViaGateway`.
    // A test on `sealText` alone would prove the sealer works and say nothing
    // about whether anything uses it.
    const { buildUpstream, sealedSecretsOf } = await import('./llm-gateway')
    const route = { endpoint: { name: 'e', provider: 'openai', baseUrl: 'https://api.test' }, upstreamModel: 'm' }
    const call = await buildUpstream(route as never, {
      messages: [
        { role: 'system', content: 'you are helpful' },
        { role: 'user', content: `deploy with ${PAT} please` },
      ],
    })

    const wire = JSON.stringify(call.body)
    expect(wire).not.toContain(PAT)
    expect(wire).toContain('«secret:1»')
    // The audit line names the KIND and never the value.
    expect(sealedSecretsOf(call).map((s) => s.label)).toEqual(['GitHub fine-grained token'])
    expect(JSON.stringify(sealedSecretsOf(call))).not.toContain(PAT)
    // And the value is still recoverable at a genuine boundary.
    expect(unsealText('«secret:1»', call.vault!)).toBe(PAT)
  })

  it('leaves an ordinary conversation byte-for-byte alone', async () => {
    // A layer on the hot path of every model call has to be invisible when there
    // is nothing to do, or it becomes the thing somebody switches off.
    const { buildUpstream } = await import('./llm-gateway')
    const route = { endpoint: { name: 'e', provider: 'openai', baseUrl: 'https://api.test' }, upstreamModel: 'm' }
    const messages = [{ role: 'user', content: 'summarise the ledger migration thread' }]
    const call = await buildUpstream(route as never, { messages })
    expect(call.body.messages).toEqual(messages)
    expect(call.vault?.sealed).toEqual([])
  })
})

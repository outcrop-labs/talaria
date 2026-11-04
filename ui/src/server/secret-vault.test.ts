// THE VALUE MUST NOT TRAVEL. Everything below is a property of that sentence.
//
// A business asking whether its keys are safe here deserves an answer that does
// not depend on a model's judgement, and the adversarial tier says why: on this
// install the four strongest models file `secret_leak` on two of four seeds
// each, after grounding. This layer is what makes that number not matter for
// credentials WE put in front of them.
import { describe, expect, it, vi } from 'vitest'
import { inventedHandles, newVault, sealContent, sealMessages, sealText, unsealText } from './secret-vault'

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

  it('seals the text inside ARRAY content and leaves the other parts alone', () => {
    // sealContent is what both chokepoints map messages through — prose turns
    // AND image turns have to leave the process sealed.
    const v = newVault()
    const parts = [
      { type: 'text', text: `deploy with ${PAT}` },
      { type: 'image_url', image_url: { url: 'data:image/png;base64,AA' } },
      { type: 'text', text: 'no secrets here' },
    ]
    const out = sealContent(parts, v) as Array<{ type: string; text?: string }>
    expect(out[0]?.text).not.toContain(PAT)
    expect(unsealText(out[0]!.text!, v)).toBe(`deploy with ${PAT}`)
    expect(out[2]?.text).toBe('no secrets here')
    // Non-text parts pass through by reference — nothing to seal, nothing to copy.
    expect(out[1]).toBe(parts[1])
    // And a plain string still seals, same as before the array case existed.
    expect(sealContent(`key: ${KEY}`, v)).not.toContain(KEY)
  })
})

// ── The chokepoint ───────────────────────────────────────────────────────────
//
// Every model call this process makes now goes to the Rust relay (harness/
// transport.ts), whose `build_upstream` seals each message on the way out and
// whose persona door (`proxy_chat`) seals an agent turn — both proven in
// api/crates/talaria-gateway (vault.rs and its callers). The sealer here is
// still what the guard and the workspace-secret surfaces use.

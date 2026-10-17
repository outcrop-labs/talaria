import { describe, expect, it } from 'vitest'
import { addDomain, deadLine, featuredChips, hiddenDeadLine, isBareHost, livenessLine, showDeadLabel, toggleChip } from './mcp-catalog'

const rel = () => '3h ago'

describe('livenessLine', () => {
  it('says nothing before the first check, and that checks are due when rows wait', () => {
    expect(livenessLine(undefined)).toBe('')
    expect(livenessLine({ checked: 0, dead: 0, unknown: 0, pending: 0 })).toBe('')
    expect(livenessLine({ checked: 0, dead: 0, unknown: 0, pending: 12 })).toBe('liveness not checked yet')
  })
  it('counts checked, unreachable and remaining', () => {
    expect(livenessLine({ checked: 1200, dead: 34, unknown: 5, pending: 26000 })).toBe(
      `${(1200).toLocaleString()} checked · 34 unreachable · ${(26000).toLocaleString()} to go`,
    )
    expect(livenessLine({ checked: 10, dead: 0, unknown: 0, pending: 0 })).toBe('10 checked')
  })
})

describe('hiddenDeadLine', () => {
  it('is null when nothing was hidden, singular for one, plural otherwise', () => {
    expect(hiddenDeadLine(undefined)).toBeNull()
    expect(hiddenDeadLine(0)).toBeNull()
    expect(hiddenDeadLine(1)).toBe('1 unreachable server hidden')
    expect(hiddenDeadLine(12)).toBe('12 unreachable servers hidden')
  })
})

describe('showDeadLabel', () => {
  it('names the count, never the status sentence', () => {
    expect(showDeadLabel(1)).toBe('Show 1 unreachable')
    expect(showDeadLabel(12)).toBe('Show 12 unreachable')
    expect(showDeadLabel(1200)).toBe(`Show ${(1200).toLocaleString()} unreachable`)
    expect(showDeadLabel(0)).toBe('Show unreachable')
    expect(showDeadLabel(undefined)).toBe('Show unreachable')
  })
})

describe('deadLine', () => {
  it('only a dead verdict gets the line, with when it was checked', () => {
    expect(deadLine(null, rel)).toBeNull()
    expect(deadLine({ state: 'ok', checkedAt: 1, detail: null }, rel)).toBeNull()
    expect(deadLine({ state: 'auth', checkedAt: 1, detail: null }, rel)).toBeNull()
    expect(deadLine({ state: 'unknown', checkedAt: 1, detail: null }, rel)).toBeNull()
    expect(deadLine({ state: 'dead', checkedAt: 1, detail: 'HTTP 404' }, rel)).toBe('Unreachable when last checked 3h ago')
    expect(deadLine({ state: 'dead', checkedAt: null, detail: null }, rel)).toBe('Unreachable when last checked')
  })
})

describe('isBareHost', () => {
  it('matches the server rule', () => {
    for (const ok of ['linear.app', 'mcp.deepwiki.com', 'Deepwiki.COM.', 'a-b.co']) expect(isBareHost(ok)).toBe(true)
    for (const bad of ['https://linear.app', 'linear.app/mcp', 'linear.app:443', 'localhost', '*.x.com', '-x.com', '10.0.0.1', ''])
      expect(isBareHost(bad)).toBe(false)
  })
})

describe('the featured chips', () => {
  const builtins = ['github.com', 'paypal.com']
  const empty = { domains: [], hidden: [], standIns: [] }

  it('lists built-ins then added, marking each, hidden ones still present', () => {
    const chips = featuredChips(builtins, { domains: ['deepwiki.com', 'github.com'], hidden: ['paypal.com'], standIns: [] })
    expect(chips).toEqual([
      { domain: 'github.com', builtin: true, hidden: false },
      { domain: 'paypal.com', builtin: true, hidden: true },
      { domain: 'deepwiki.com', builtin: false, hidden: false },
    ])
  })

  it('removing a built-in hides it, removing an added one drops it, and a hidden one restores', () => {
    const hid = toggleChip(builtins, empty, 'paypal.com')
    expect(hid.hidden).toEqual(['paypal.com'])
    expect(toggleChip(builtins, hid, 'paypal.com').hidden).toEqual([])
    const added = addDomain(empty, ' DeepWiki.com ')
    expect(added.domains).toEqual(['deepwiki.com'])
    expect(addDomain(added, 'deepwiki.com').domains).toEqual(['deepwiki.com'])
    expect(toggleChip(builtins, added, 'deepwiki.com').domains).toEqual([])
    // Adding a hidden domain back un-hides it.
    expect(addDomain(hid, 'paypal.com').hidden).toEqual([])
  })
})

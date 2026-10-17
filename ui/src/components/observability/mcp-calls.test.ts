import { describe, expect, it } from 'vitest'
import {
  OUTCOMES,
  OUTCOME_META,
  WINDOWS,
  callsUrl,
  cardLine,
  errorRate,
  formatLatency,
  isOutcome,
  isWindow,
  panelHref,
  RETENTION_CHOICES,
  retentionLabel,
  retentionOptions,
  serverCalls,
  sinceFor,
  summaryUrl,
  type McpCallsSummary,
} from './mcp-calls'

describe('outcomes', () => {
  it('names the api’s seven outcomes, each with a label and a tone', () => {
    expect([...OUTCOMES]).toEqual(['ok', 'error', 'denied', 'not_connected', 'upstream_error', 'spend_error', 'cancelled'])
    for (const o of OUTCOMES) {
      expect(OUTCOME_META[o].label).toBeTruthy()
      expect(isOutcome(o)).toBe(true)
    }
    expect(OUTCOME_META.ok.tone).toBe('success')
    expect(OUTCOME_META.denied.tone).toBe('warn')
    expect(OUTCOME_META.not_connected.tone).toBe('neutral')
    // A caller hanging up is not the server failing.
    expect(OUTCOME_META.cancelled.tone).toBe('neutral')
    for (const failure of ['error', 'upstream_error', 'spend_error'] as const) expect(OUTCOME_META[failure].tone).toBe('danger')
    expect(isOutcome('bogus')).toBe(false)
    expect(isOutcome(null)).toBe(false)
  })
})

describe('formatLatency', () => {
  it.each([
    [null, '—'],
    [undefined, '—'],
    [Number.NaN, '—'],
    [0, '0 ms'],
    [12.4, '12 ms'],
    [999, '999 ms'],
    [1000, '1.0 s'],
    [1449, '1.4 s'],
    [12_345, '12 s'],
    [59_999, '60 s'],
    [60_000, '1m 00s'],
    [125_000, '2m 05s'],
    [-5, '0 ms'],
  ])('%s → %s', (ms, out) => {
    expect(formatLatency(ms as number | null | undefined)).toBe(out)
  })
})

describe('cardLine', () => {
  it('reads like the spec line', () => {
    expect(cardLine({ calls: 142, errors: 3, denied: 1, notConnected: 0 })).toBe('Last 24h: 142 calls · 3 errors · 1 denied')
  })
  it('always says calls and errors, refusals and missing connections only when present', () => {
    expect(cardLine({ calls: 1, errors: 0, denied: 0, notConnected: 0 })).toBe('Last 24h: 1 call · 0 errors')
    expect(cardLine({ calls: 2, errors: 1, denied: 0, notConnected: 2 })).toBe('Last 24h: 2 calls · 1 error · 2 not connected')
    expect(cardLine({ calls: 1234, errors: 0, denied: 0, notConnected: 0 })).toBe('Last 24h: 1,234 calls · 0 errors')
  })
  it('says no calls for a quiet or absent server', () => {
    expect(cardLine(null)).toBe('Last 24h: no calls')
    expect(cardLine({ calls: 0, errors: 0, denied: 0, notConnected: 0 })).toBe('Last 24h: no calls')
    expect(cardLine(null, 'Last hour')).toBe('Last hour: no calls')
  })
})

describe('urls', () => {
  it('leaves blank filters out of the list url', () => {
    expect(callsUrl({})).toBe('/api/mcp/calls')
    expect(callsUrl({ server: '', agent: null, outcome: null })).toBe('/api/mcp/calls')
    expect(callsUrl({ server: 'notion', agent: 'ava-ops', tool: 'search', outcome: 'denied', since: 1000, limit: 50 })).toBe(
      '/api/mcp/calls?server=notion&agent=ava-ops&tool=search&outcome=denied&since=1000&limit=50',
    )
    expect(callsUrl({ since: 0 })).toBe('/api/mcp/calls?since=0')
  })
  it('builds the summary url, optionally for one server', () => {
    expect(summaryUrl(5)).toBe('/api/mcp/calls/summary?since=5')
    expect(summaryUrl(5, 'a b')).toBe('/api/mcp/calls/summary?since=5&server=a+b')
  })
  it('deep-links a card into the tab filtered to its server', () => {
    expect(panelHref('my server')).toBe('/observability/mcp?server=my%20server')
  })
})

describe('windows', () => {
  it('floors the window start to the minute, so a poll keeps its key', () => {
    const now = Date.UTC(2026, 9, 16, 12, 30, 45, 123)
    expect(sinceFor('1h', now)).toBe(Date.UTC(2026, 9, 16, 11, 30, 0, 0))
    expect(sinceFor('24h', now)).toBe(Date.UTC(2026, 9, 15, 12, 30, 0, 0))
    expect(sinceFor('7d', now)).toBe(Date.UTC(2026, 9, 9, 12, 30, 0, 0))
    expect(sinceFor('1h', now + 10_000)).toBe(sinceFor('1h', now))
  })
  it('knows its three windows', () => {
    expect(WINDOWS.map((w) => w.id)).toEqual(['1h', '24h', '7d'])
    expect(isWindow('24h')).toBe(true)
    expect(isWindow('30d')).toBe(false)
    expect(isWindow(null)).toBe(false)
  })
})

describe('serverCalls / errorRate', () => {
  const summary: McpCallsSummary = {
    since: '2026-10-15T00:00:00.000Z',
    totals: { calls: 3, errors: 1, denied: 0, notConnected: 0 },
    servers: [
      { serverId: 'id-1', serverName: 'notion', calls: 3, errors: 1, denied: 0, notConnected: 0, p50Ms: 10, p95Ms: 20, topTools: [] },
    ],
    agents: [],
  }
  it('finds a server by id or by name', () => {
    expect(serverCalls(summary, 'id-1')?.serverName).toBe('notion')
    expect(serverCalls(summary, 'notion')?.serverId).toBe('id-1')
    expect(serverCalls(summary, 'github')).toBeNull()
    expect(serverCalls(undefined, 'notion')).toBeNull()
  })
  it('renders an error share', () => {
    expect(errorRate({ calls: 0, errors: 0 })).toBe('—')
    expect(errorRate({ calls: 10, errors: 0 })).toBe('0%')
    expect(errorRate({ calls: 3, errors: 1 })).toBe('33%')
    expect(errorRate({ calls: 1000, errors: 21 })).toBe('2.1%')
  })
})

describe('retention', () => {
  it('offers the fixed choices, plus a stored value outside them', () => {
    expect(retentionOptions(30)).toEqual([...RETENTION_CHOICES])
    expect(retentionOptions(null)).toEqual([...RETENTION_CHOICES])
    expect(retentionOptions(45)).toEqual([7, 14, 30, 45, 90, 180, 365])
    expect(retentionOptions(3650)).toEqual([7, 14, 30, 90, 180, 365, 3650])
    for (const bad of [0, -3, 2.5, Number.NaN]) expect(retentionOptions(bad)).toEqual([...RETENTION_CHOICES])
  })
  it.each([
    [1, '1 day'],
    [7, '7 days'],
    [30, '30 days'],
    [365, '1 year'],
    [3650, '3,650 days'],
  ])('labels %d as %s', (d, label) => {
    expect(retentionLabel(d)).toBe(label)
  })
})

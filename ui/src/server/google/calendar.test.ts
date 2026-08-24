import { afterEach, describe, expect, it, vi } from 'vitest'
import { listUpcomingEventsWithToken } from '@/server/google/calendar'

// The agenda read. The shape under test is what Google's events.list really
// returns once a calendar uses the newer event kinds: every entry carries an
// eventType, and working locations ("Office", repeating all day every weekday)
// arrive as ordinary-looking all-day events that a naive listing treats as
// meetings. The assertions pin that they are dropped — without shrinking the
// agenda — while real events (default, focus time, out of office) survive.

const event = (id: string, over: Partial<{ summary: string; eventType: string }> = {}) => ({
  id,
  summary: over.summary ?? id,
  start: { dateTime: '2026-08-24T10:00:00Z' },
  end: { dateTime: '2026-08-24T11:00:00Z' },
  htmlLink: `https://calendar.google.com/${id}`,
  attendees: [],
  ...over,
})

const page = (items: unknown[]) =>
  new Response(JSON.stringify({ items }), { status: 200 })

afterEach(() => vi.unstubAllGlobals())

describe('listUpcomingEventsWithToken', () => {
  it('drops working locations, keeps real events', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () =>
        page([
          { ...event('wl-1', { summary: 'Office' }), eventType: 'workingLocation', start: { date: '2026-08-24' }, end: { date: '2026-08-24' } },
          event('standup'),
          { ...event('focus-1', { summary: 'Deep work' }), eventType: 'focusTime' },
          { ...event('ooo-1', { summary: 'Out of office' }), eventType: 'outOfOffice' },
        ]),
      ),
    )
    const events = await listUpcomingEventsWithToken('t', Date.now(), 10)
    expect(events.map((e) => e.id)).toEqual(['standup', 'focus-1', 'ooo-1'])
  })

  it('over-fetches so dropped locations do not shrink the agenda', async () => {
    const fetched = vi.fn(async (_url: unknown) =>
      page([
        // A whole week of "Office" plus two real meetings.
        ...[0, 1, 2, 3, 4].map((i) => ({
          ...event(`wl-${i}`, { summary: 'Office' }),
          eventType: 'workingLocation',
          start: { date: `2026-08-2${4 + i}` },
          end: { date: `2026-08-2${4 + i}` },
        })),
        event('standup'),
        event('retro'),
      ]),
    )
    vi.stubGlobal('fetch', fetched)
    const events = await listUpcomingEventsWithToken('t', Date.now(), 2)
    // The two slots the caller asked for are both REAL events — the five
    // locations fetched ahead of them never reach the agenda.
    expect(events.map((e) => e.id)).toEqual(['standup', 'retro'])
    // And the over-fetch actually happened (2 wanted → 6 requested).
    const asked = new URL(fetched.mock.calls[0]![0] as unknown as string).searchParams.get('maxResults')
    expect(asked).toBe('6')
  })
})

// Google Calendar service — read the connected user's upcoming agenda and
// create events, acting strictly as that user (per-user OAuth).

import { getAccessToken } from './connections'

const CAL_BASE = 'https://www.googleapis.com/calendar/v3/calendars/primary/events'

async function requireToken(userId: string, nowMs: number): Promise<string> {
  const token = await getAccessToken(userId, nowMs)
  if (!token) {
    const err = new Error('not_connected')
    err.name = 'GoogleNotConnected'
    throw err
  }
  return token
}

export interface CalendarEvent {
  id: string
  summary: string
  /** RFC3339 dateTime, or a date (all-day). */
  start: string | null
  end: string | null
  allDay: boolean
  location: string | null
  htmlLink: string | null
  attendees: string[]
}

function normalize(e: {
  id: string
  summary?: string
  start?: { dateTime?: string; date?: string }
  end?: { dateTime?: string; date?: string }
  location?: string
  htmlLink?: string
  attendees?: Array<{ email?: string }>
}): CalendarEvent {
  const allDay = !!e.start?.date && !e.start?.dateTime
  return {
    id: e.id,
    summary: e.summary ?? '(no title)',
    start: e.start?.dateTime ?? e.start?.date ?? null,
    end: e.end?.dateTime ?? e.end?.date ?? null,
    allDay,
    location: e.location ?? null,
    htmlLink: e.htmlLink ?? null,
    attendees: (e.attendees ?? []).map((a) => a.email ?? '').filter(Boolean),
  }
}

/** Upcoming events (from now), soonest first. */
export async function listUpcomingEvents(userId: string, nowMs: number, maxResults = 10): Promise<CalendarEvent[]> {
  const token = await requireToken(userId, nowMs)
  const params = new URLSearchParams({
    timeMin: new Date(nowMs).toISOString(),
    maxResults: String(Math.min(Math.max(maxResults, 1), 50)),
    singleEvents: 'true',
    orderBy: 'startTime',
  })
  const res = await fetch(`${CAL_BASE}?${params.toString()}`, { headers: { Authorization: `Bearer ${token}` } })
  if (!res.ok) throw new Error(`calendar list failed: ${res.status} ${await res.text()}`)
  const data = (await res.json()) as { items?: Parameters<typeof normalize>[0][] }
  return (data.items ?? []).map(normalize)
}

export interface CreateEventInput {
  summary: string
  description?: string
  location?: string
  /** RFC3339 dateTime (timed) or YYYY-MM-DD (all-day). */
  start: string
  end: string
  allDay?: boolean
  attendees?: string[]
}

/** Create an event on the user's primary calendar. */
export async function createEvent(userId: string, nowMs: number, input: CreateEventInput): Promise<CalendarEvent> {
  const token = await requireToken(userId, nowMs)
  const timeField = input.allDay ? 'date' : 'dateTime'
  const body = {
    summary: input.summary,
    description: input.description,
    location: input.location,
    start: { [timeField]: input.start },
    end: { [timeField]: input.end },
    attendees: input.attendees?.map((email) => ({ email })),
  }
  const res = await fetch(`${CAL_BASE}?sendUpdates=all`, {
    method: 'POST',
    headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  })
  if (!res.ok) throw new Error(`calendar create failed: ${res.status} ${await res.text()}`)
  return normalize((await res.json()) as Parameters<typeof normalize>[0])
}

// Google Calendar service — read the connected user's upcoming agenda and
// create events, acting strictly as that user (per-user OAuth).

import { requireToken } from './connections'

const eventsUrl = (calendarId = 'primary') =>
  `https://www.googleapis.com/calendar/v3/calendars/${encodeURIComponent(calendarId || 'primary')}/events`

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

// Working locations ("at the office Mon–Fri") are Calendar events under the
// hood — eventType 'workingLocation' — and they repeat all day every weekday,
// so with singleEvents expansion one of them eats a big share of a 10-slot
// agenda. They are where you'll be, not what you're doing: an agenda lists
// commitments. focusTime and outOfOffice stay — those ARE commitments.
const AGENDA_EVENT_TYPES = new Set(['default', 'focusTime', 'outOfOffice'])

/** Upcoming events (from now), soonest first. */
export async function listUpcomingEvents(userId: string, nowMs: number, maxResults = 10): Promise<CalendarEvent[]> {
  return listUpcomingEventsWithToken(await requireToken(userId, nowMs), nowMs, maxResults)
}

/** Upcoming events using an already-resolved token (per-user or org). */
export async function listUpcomingEventsWithToken(token: string, nowMs: number, maxResults = 10, calendarId?: string | null): Promise<CalendarEvent[]> {
  const wanted = Math.min(Math.max(maxResults, 1), 50)
  const params = new URLSearchParams({
    timeMin: new Date(nowMs).toISOString(),
    // Over-fetch 3× so dropped working locations don't shrink the agenda —
    // they were fetched, they just don't count against the slots.
    maxResults: String(Math.min(wanted * 3, 50)),
    singleEvents: 'true',
    orderBy: 'startTime',
  })
  const res = await fetch(`${eventsUrl(calendarId ?? undefined)}?${params.toString()}`, { headers: { Authorization: `Bearer ${token}` } })
  if (!res.ok) throw new Error(`calendar list failed: ${res.status} ${await res.text()}`)
  const data = (await res.json()) as { items?: Array<Parameters<typeof normalize>[0] & { eventType?: string }> }
  return (data.items ?? [])
    .filter((e) => AGENDA_EVENT_TYPES.has(e.eventType ?? 'default'))
    .slice(0, wanted)
    .map(normalize)
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
  return createEventWithToken(await requireToken(userId, nowMs), input)
}

/** Create an event using an already-resolved token (per-user or org). */
export async function createEventWithToken(token: string, input: CreateEventInput, calendarId?: string | null): Promise<CalendarEvent> {
  const timeField = input.allDay ? 'date' : 'dateTime'
  const body = {
    summary: input.summary,
    description: input.description,
    location: input.location,
    start: { [timeField]: input.start },
    end: { [timeField]: input.end },
    attendees: input.attendees?.map((email) => ({ email })),
  }
  const res = await fetch(`${eventsUrl(calendarId ?? undefined)}?sendUpdates=all`, {
    method: 'POST',
    headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  })
  if (!res.ok) throw new Error(`calendar create failed: ${res.status} ${await res.text()}`)
  return normalize((await res.json()) as Parameters<typeof normalize>[0])
}

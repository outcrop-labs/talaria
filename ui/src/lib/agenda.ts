// Agenda day-grouping — pure, client-side, and deliberately NOT under
// src/routes (vitest excludes routes/, and this is exactly the logic worth
// testing: local-midnight splits and the all-day UTC trap). The Home agenda
// reads by day — Today · Tomorrow · the following days — so the panel groups
// rather than asking every row to restate its own date. Local dates, because
// the browser's timezone is the right one for a person's own agenda.

export interface AgendaEvent {
  id: string
  summary: string
  start: string | null
  end: string | null
  allDay: boolean
  location: string | null
  htmlLink: string | null
}

export interface AgendaDay {
  /** Local `yyyy-mm-dd` — the grouping key, independent of the label. */
  key: string
  /** Today · Tomorrow · Wednesday, Aug 27 */
  label: string
  events: AgendaEvent[]
}

const dayKeyOf = (d: Date) =>
  `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`

export function groupAgendaByDay(events: AgendaEvent[], now = new Date()): AgendaDay[] {
  const todayKey = dayKeyOf(now)
  const tomorrowKey = dayKeyOf(new Date(now.getFullYear(), now.getMonth(), now.getDate() + 1))
  const byKey = new Map<string, AgendaEvent[]>()
  for (const e of events) {
    if (!e.start) continue
    // A date-only start IS the day; `new Date('2026-08-26')` is UTC midnight,
    // which files it under Aug 25 in every timezone behind UTC. Take the
    // literal date; parse anything longer as the instant it names.
    const key =
      e.allDay && /^\d{4}-\d{2}-\d{2}$/.test(e.start) ? e.start : dayKeyOf(new Date(e.start))
    const bucket = byKey.get(key)
    if (bucket) bucket.push(e)
    else byKey.set(key, [e])
  }
  return [...byKey.entries()]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([key, dayEvents]) => ({
      key,
      label:
        key === todayKey
          ? 'Today'
          : key === tomorrowKey
            ? 'Tomorrow'
            : // Local midnight of the key — never UTC — so the weekday matches.
              new Date(`${key}T00:00:00`).toLocaleDateString(undefined, {
                weekday: 'long',
                month: 'short',
                day: 'numeric',
              }),
      events: dayEvents,
    }))
}

/** The time column inside a day group — the date lives on the group header. */
export function formatAgendaTime(e: AgendaEvent): string {
  if (!e.start) return ''
  if (e.allDay) return 'All day'
  return new Date(e.start).toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' })
}

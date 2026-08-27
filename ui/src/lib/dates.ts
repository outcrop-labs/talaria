// Date <-> instant conversion. ONE convention, used by every surface that edits
// a schedule date, because `due_date`/`start_date` are timestamptz (an instant)
// while the UI only ever asks the user for a calendar DATE.
//
// A user who types "due Aug 5" means the end of the working day on Aug 5 WHERE
// THEY ARE — so we anchor a due date at 17:00 LOCAL and a start date at 09:00
// LOCAL. That matches the Today/Tomorrow/Next-week quick-picks, the Gantt drag
// handles, and the board's "due today" filter (which uses local end-of-day).
//
// The trap these helpers exist to close: `new Date('2026-08-05')` parses a
// date-ONLY string as UTC midnight, which anywhere west of Greenwich is the
// *previous* evening — the ticket then renders a day early and reads as overdue
// a day early. Symmetrically, `iso.slice(0, 10)` reads back the UTC date, so a
// 17:00-local instant round-trips into the picker as the NEXT day. Always go
// through the pair below; never hand a raw ISO slice to an <input type="date">.
//
// This lives in lib/ rather than beside the pills that use it because it is
// pure date math with a timezone contract, and a contract that cannot be tested
// is a contract nobody is keeping. It was in a .tsx component file, which put
// it out of reach of a node-environment test.

/** Hour-of-day (local) each schedule date is anchored at. */
const DUE_HOUR = 17
const START_HOUR = 9

/** `YYYY-MM-DD` (from an <input type="date">) -> ISO instant at `hour` LOCAL.
 *  Returns null for an unparseable value so callers never persist garbage —
 *  `new Date('nonsense').toISOString()` throws RangeError inside onChange. */
const localDateAtHour = (ymd: string, hour: number): string | null => {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(ymd)
  if (!m) return null
  const [y, mo, day] = [Number(m[1]), Number(m[2]), Number(m[3])]
  const d = new Date(y, mo - 1, day, hour, 0, 0, 0)
  if (Number.isNaN(d.getTime())) return null
  // The Date constructor ROLLS OVER out-of-range parts rather than failing:
  // `new Date(2026, 12, 45)` is 2027-02-14, not an error. The shape check above
  // passes for "2026-13-45", so without this a nonsense day became a real date
  // a year away — silently, and stored. Round-tripping the parts is the only
  // way to tell "that day exists" from "that day was invented for me".
  if (d.getFullYear() !== y || d.getMonth() !== mo - 1 || d.getDate() !== day) return null
  return d.toISOString()
}

/** Due date picked in an <input type="date"> -> stored instant (17:00 local). */
export const dueIsoFromDateInput = (ymd: string): string | null => localDateAtHour(ymd, DUE_HOUR)

/** Start date picked in an <input type="date"> -> stored instant (09:00 local). */
export const startIsoFromDateInput = (ymd: string): string | null => localDateAtHour(ymd, START_HOUR)

/** Stored instant -> the `YYYY-MM-DD` an <input type="date"> should show. Uses
 *  the LOCAL calendar date, matching how the pill/Gantt render it. `slice(0, 10)`
 *  would use the UTC date and be off by one for most of the world. */
export const dateInputValue = (iso: string | null | undefined): string => {
  if (!iso) return ''
  const d = new Date(iso)
  if (Number.isNaN(d.getTime())) return ''
  const p = (n: number) => String(n).padStart(2, '0')
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`
}

/** `n` days from now, anchored at the due hour — the Today/Tomorrow/Next-week
 *  quick-picks. Relative to the local day, not to 24h multiples, so it lands on
 *  the right calendar date across a DST boundary. */
export const dueIsoInDays = (days: number): string => {
  const d = new Date()
  d.setDate(d.getDate() + days)
  d.setHours(DUE_HOUR, 0, 0, 0)
  return d.toISOString()
}

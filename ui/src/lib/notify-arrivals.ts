/** Which notification rows this tab has already surfaced, so live arrivals
 *  become toasts exactly once.
 *
 *  The FIRST read is the baseline, empty or not: a reload must not replay the
 *  inbox as a burst of toasts. Only rows that land after that first read are
 *  arrivals — including the first row ever, when the baseline was an empty
 *  feed.
 *
 *  Framework-free so it is testable without a DOM. */
export class ArrivalTracker<T extends { id: string }> {
  private seen = new Set<string>()
  private primed = false

  constructor(private readonly maxSeen = 1_000) {}

  /** Feed it a page (newest first, as /api/notifications returns it); get
   *  back the rows never seen before, oldest first — the order they happened,
   *  which is the order to toast them in. */
  arrive(rows: readonly T[]): T[] {
    if (!this.primed) {
      this.primed = true
      for (const r of rows) this.seen.add(r.id)
      return []
    }
    const fresh: T[] = []
    for (const r of rows) {
      if (this.seen.has(r.id)) continue
      this.seen.add(r.id)
      fresh.push(r)
    }
    // Long sessions shouldn't grow this without bound. The feed window only
    // ever holds the recent rows, so ids outside it can never reappear;
    // rebuilding from the current page keeps the set proportional to the
    // feed, not the session.
    if (this.seen.size > this.maxSeen) this.seen = new Set(rows.map((r) => r.id))
    return fresh.reverse()
  }
}

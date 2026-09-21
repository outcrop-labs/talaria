// Adopt-once bookkeeping for the silent timezone detection. The flag exists
// for exactly one case: a person who CLEARED their zone on purpose ("follow
// the workspace zone") must not have the browser's zone re-saved on the next
// load. Without it, adoption would treat every deliberate clear as a blank
// to fill, and the clear would last one reload.
//
// Per-user key, because a shared machine's browsers are different people and
// one person's flag must not silence another's first-run adoption. Storage
// failures are absorbed rather than surfaced: an unmarkable flag only means
// adoption re-runs, and its guard (`timezone === null`) is already false by
// then — the PUT is idempotent, not a loop.

import { readFlag, writeFlag } from '@/lib/persist'

const key = (userId: string) => `talaria.tz.adopted.${userId}`

export function hasAdopted(userId: string): boolean {
  return readFlag(key(userId))
}

export function markTimezoneAdopted(userId: string): void {
  writeFlag(key(userId), true)
}

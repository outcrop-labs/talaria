// The one fact only the boot process knows: whether the migration pass
// survived. server-entry.js records a failure on globalThis (the same channel
// pg.ts uses for its pool handle and migration promise), and /api/healthz
// turns it into a check — so a failed pass fails the probe (compose
// healthcheck, deploy gates) instead of leaving a green container that 500s
// every table query. A slow-but-running pass records nothing: slow is not
// failed, and that path is a boot-time warning, not an outage.

export interface BootMigrationError {
  /** Safe short code only — healthz's body law: no driver messages. */
  code: string
  at: number
}

/** The `migrations` check for /api/healthz — null when there is nothing to
 *  report (the overwhelmingly common case; the check appears only when it has
 *  something to say, same as the conditional rustApi check). */
export function bootMigrationCheck(): { ok: false; latencyMs: null; error: string } | null {
  const err = (globalThis as { __talariaBootMigrationError?: BootMigrationError }).__talariaBootMigrationError
  return err ? { ok: false, latencyMs: null, error: err.code } : null
}

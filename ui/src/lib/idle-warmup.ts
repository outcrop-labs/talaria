/**
 * Fire-and-forget warm-up for when the document goes idle, as an `$effect`
 * cleanup contract: run `task` once (idle callback where the platform ships
 * one, a 250ms timeout where it does not), return the cancel function.
 *
 * `typeof`, never a bare reference: WebKit (Safari — and WebKitGTK, the
 * desktop shell's engine) never shipped requestIdleCallback, and reading an
 * absent global by name throws ReferenceError before a `??` fallback can run
 * ("Can't find variable: requestIdleCallback" was exactly that). Swallow,
 * don't surface twice: a failing warm-up is already reported by the router's
 * onError hook (the recovery banner lives there); the catch only keeps this
 * fire-and-forget from ALSO raising an unhandled rejection.
 *
 * Dependency-free on purpose (no router import): the unit test exercises both
 * scheduling paths without loading the route tree.
 */
export function scheduleIdleWarmup(task: () => Promise<unknown>): () => void {
  const run = () => void task().catch(() => {})
  if (typeof requestIdleCallback !== 'function') {
    const t = setTimeout(run, 250)
    return () => clearTimeout(t)
  }
  const id = requestIdleCallback(run)
  return () => cancelIdleCallback(id)
}

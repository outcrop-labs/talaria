// A stand-in for `@oh-my-pi/pi-natives`, substituted at bundle time by
// build.ts.
//
// WHY THIS EXISTS. `@oh-my-pi/pi-ai` carries omp's OAuth flows, and reaching
// them pulls `@oh-my-pi/pi-natives` — whose platform package is 364 MB of
// prebuilt binaries with no musl build at all. Talaria's app image is alpine,
// so shipping it is both impossible and absurd: two orders of magnitude of
// image weight for code no login flow on this host ever calls.
//
// WHAT IS ACTUALLY NATIVE in the graph we import: a custom-URL-scheme OAuth
// callback receiver (for `omp://`-style redirects — we never register one), a
// Windows long-path helper, advisory file locks (ours is the only process
// touching these credentials, and it holds them in Postgres, not a file), a
// native process manager, a mermaid renderer, and Apple Foundation Models.
// None of it participates in an authorization-code, device-code or paste-code
// flow on Linux, which is every flow this bridge runs.
//
// THE CONTRACT. Every stub below either answers honestly (the Windows path
// helper is identity on Linux; Apple FM is unavailable) or THROWS. It never
// pretends to have done something. If a future omp release routes a login
// through one of these, the login fails loudly with the symbol's name in the
// message instead of silently misbehaving — and that failure is the signal to
// revisit this file, not to widen a stub.
//
// The file lock is the one deliberate no-op: it guards a credential file this
// bridge does not keep, and a lock that is never contended is correctly a
// no-op. The acquire/release pair stays so the call sites' control flow is
// unchanged.

const unavailable = (what: string) => (): never => {
  throw new Error(`${what} is unavailable in Talaria's omp auth bridge (native binding stubbed)`)
}

/** Custom-URL-scheme callback receiver. We never advertise a native scheme. */
export class NativeOAuthCallback {
  constructor() {
    unavailable('native-scheme OAuth callback')()
  }
}

/** Advisory file lock over a credential file this bridge does not keep. */
export class FileLock {
  static async acquire(): Promise<FileLock> {
    return new FileLock()
  }
  static acquireSync(): FileLock {
    return new FileLock()
  }
  async release(): Promise<void> {}
  releaseSync(): void {}
}

export class Process {
  constructor() {
    unavailable('native process manager')()
  }
}

export const ProcessStatus = { Running: 'running', Exited: 'exited' } as const

export type MermaidRenderOptions = Record<string, unknown>
export const renderMermaidAscii = unavailable('mermaid rendering')

export const appleFmAvailability = (): { available: false } => ({ available: false })
export const appleFmCancel = (): void => {}
export const appleFmGenerate = unavailable('Apple Foundation Models')

/** Identity on every platform this image runs on. */
export function expandWindowsLongPath(p: string): string {
  return p
}

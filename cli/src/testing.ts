// Test scaffolding — a fake Ctx with planted answers. This is the
// updater.test.ts idiom (answers keyed by command), injected instead of
// vi.mock'd, which is why the CLI's tests run on plain bun:test.
//
//   const ctx = fakeCtx()
//   ctx.plant(['docker', 'inspect', 'x'], '')            // success
//   ctx.plant(['git', 'status'], new Error('boom'))      // failure
//   ... run the command ...
//   ctx.calls.map(c => c.args)                            // assertions

import type { Ctx } from './ctx'
import type { Log } from './ui'
import { CliError } from './ui'

export type ExecCall = { cmd: string; args: string[]; opts?: { cwd?: string } }

export type FakeCtx = Ctx & {
  calls: ExecCall[]
  logLines: { kind: keyof Log; msg: string }[]
  /** Plant an answer for an exact (cmd, args) pair; Error rejects/runs 1. */
  plant: (cmdArgs: [string, string[]], out: string | Error) => void
}

export function fakeCtx(init: { env?: Record<string, string>; isTTY?: boolean } = {}): FakeCtx {
  const calls: ExecCall[] = []
  const logLines: FakeCtx['logLines'] = []
  const answers = new Map<string, string | Error>()
  const key = (cmd: string, args: string[]) => JSON.stringify([cmd, args])
  const answer = (cmd: string, args: string[]): string | Error | undefined =>
    answers.get(key(cmd, args))

  const log: Log = {
    say: (m) => logLines.push({ kind: 'say', msg: m }),
    ok: (m) => logLines.push({ kind: 'ok', msg: m }),
    skip: (m) => logLines.push({ kind: 'skip', msg: m }),
    warn: (m) => logLines.push({ kind: 'warn', msg: m }),
    die: (m) => {
      throw new CliError(m)
    },
    fail: (m) => logLines.push({ kind: 'fail', msg: m }),
    raw: (m) => logLines.push({ kind: 'raw', msg: m }),
  }

  return {
    root: '/repo',
    exec: async (cmd, args, opts) => {
      calls.push({ cmd, args, opts })
      const a = answer(cmd, args)
      if (a instanceof Error) throw a
      return { stdout: a ?? '', stderr: '' }
    },
    run: async (cmd, args, opts) => {
      calls.push({ cmd, args, opts })
      const a = answer(cmd, args)
      return a instanceof Error ? 1 : 0
    },
    pipe: async (a, b, opts) => {
      calls.push({ cmd: a[0], args: a[1], opts })
      calls.push({ cmd: b[0], args: b[1], opts })
      for (const c of [a, b]) {
        const ans = answer(c[0], c[1])
        if (ans instanceof Error) throw ans
      }
    },
    readLine: async () => {
      throw new Error('fakeCtx.readLine: plant nothing — tests should not prompt')
    },
    env: { ...init.env },
    isTTY: init.isTTY ?? false,
    now: () => new Date('2026-01-01T00:00:00Z'),
    log,
    calls,
    logLines,
    plant: (cmdArgs, out) => answers.set(key(cmdArgs[0], cmdArgs[1]), out),
  }
}

// The Ctx — everything process-facing a command may touch, injected. This is
// the whole testability story: commands are `run(ctx, args)` and tests hand
// them a fake ctx with planted exec answers (src/testing.ts), so no module
// mocking is needed anywhere. Real terminal wiring lives only in realCtx().

import { realpathSync } from 'node:fs'
import { dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { createInterface } from 'node:readline/promises'
import { exec, pipe, run, type ExecOpts, type ExecResult, type PipeOpts } from './exec'
import { makeLog, type Log } from './ui'
import { repoRoot } from './paths'

export type Ctx = {
  /** Canonical (realpath'd) repo root. */
  root: string
  /** Capture-output exec (never a shell). Rejects on failure. */
  exec: (cmd: string, args: string[], opts?: ExecOpts) => Promise<ExecResult>
  /** Inherit-stdio run; resolves the exit code. */
  run: (cmd: string, args: string[], opts?: { cwd?: string }) => Promise<number>
  /** Streaming pipeline with pipefail semantics. */
  pipe: (a: [string, string[]], b: [string, string[]], opts?: PipeOpts) => Promise<void>
  /** Interactive prompt (typed confirms). Reads stdin even without a tty. */
  readLine: (prompt: string) => Promise<string>
  env: Record<string, string | undefined>
  isTTY: boolean
  now: () => Date
  log: Log
}

export function realCtx(): Ctx {
  // This module's own location, physical spelling — see paths.ts for why a
  // symlinked invocation path must not survive into anything a box resolves.
  const here = realpathSync(dirname(fileURLToPath(import.meta.url)))
  return {
    root: repoRoot(here),
    exec,
    run,
    pipe,
    readLine: async (prompt) => {
      const rl = createInterface({ input: process.stdin, output: process.stdout })
      try {
        return await rl.question(prompt)
      } finally {
        rl.close()
      }
    },
    env: process.env,
    isTTY: process.stdin.isTTY === true,
    now: () => new Date(),
    log: makeLog(process.stdout.isTTY === true),
  }
}

// Shelling out — the three shapes the bash scripts used, as functions:
//
//   exec  capture output, never a shell            (bash `$(cmd …)`)
//   run   inherit stdio, return the exit code      (bare `cmd …`)
//   pipe  stream a.stdout → b.stdin, pipefail      (bash `a | b`)
//
// exec is a port of ui/src/server/updater.ts's helper (30s default timeout,
// 8MB maxBuffer, args never touch a shell). pipe MUST stream: the pg_dump |
// psql seed move pushes dumps that blow past any buffer in seconds.

import { execFile, spawn } from 'node:child_process'

export type ExecResult = { stdout: string; stderr: string }
export type ExecOpts = { cwd?: string; timeoutMs?: number }

/** Run one command, capturing output. Args never touch a shell. */
export function exec(cmd: string, args: string[], opts: ExecOpts = {}): Promise<ExecResult> {
  return new Promise((resolve, reject) => {
    execFile(cmd, args, { cwd: opts.cwd, timeout: opts.timeoutMs ?? 30_000, maxBuffer: 8 * 1024 * 1024 }, (err, stdout, stderr) => {
      if (err) reject(err instanceof Error ? err : new Error(String(err)))
      else resolve({ stdout: String(stdout), stderr: String(stderr) })
    })
  })
}

/** Run a command with the terminal's stdio, resolving its exit code. This is
 *  the `exec cmd` replacement: the child owns the tty, so Ctrl-C must reach
 *  the CHILD (which owns the foreground process group via stdio: inherit and
 *  forwards it to us as a signal) — we just translate the wait into a code,
 *  with SIGINT reporting 130 the way a shell would. */
export function run(cmd: string, args: string[], opts: { cwd?: string } = {}): Promise<number> {
  return new Promise((resolve, reject) => {
    const child = spawn(cmd, args, { stdio: 'inherit', cwd: opts.cwd })
    child.on('error', reject)
    child.on('close', (code, signal) => resolve(code ?? (signal === 'SIGINT' ? 130 : 1)))
  })
}

export type PipeOpts = { cwd?: string; quietDst?: boolean }

/** A shell pipeline without a shell: streams a's stdout into b's stdin.
 *  Rejects if EITHER side exits non-zero (pipefail semantics — a swallowed
 *  pg_dump failure would look like a successful seed). b's stdin EPIPE is
 *  expected when b dies first; it is silenced and b's own code reports why.
 *  `quietDst` discards b's STDOUT (the `| psql >/dev/null` shape): restore
 *  chatter is noise, while b's stderr still surfaces. */
export function pipe(
  a: [string, string[]],
  b: [string, string[]],
  opts: PipeOpts = {},
): Promise<void> {
  return new Promise((resolve, reject) => {
    const src = spawn(a[0], a[1], { stdio: ['ignore', 'pipe', 'pipe'], cwd: opts.cwd })
    const dst = spawn(b[0], b[1], { stdio: ['pipe', opts.quietDst ? 'ignore' : 'inherit', 'inherit'], cwd: opts.cwd })
    src.on('error', reject)
    dst.on('error', reject)
    dst.stdin.on('error', () => {}) // EPIPE when dst exits first — surfaced below
    src.stdout.pipe(dst.stdin)

    let srcCode: number | null = null
    let dstCode: number | null = null
    let srcErr: Error | null = null
    let dstErr: Error | null = null
    const srcErrChunks: Buffer[] = []
    src.stderr.on('data', (d) => srcErrChunks.push(d))

    const settle = () => {
      if (srcCode === null || dstCode === null) return
      if (srcCode !== 0) reject(srcErr ?? new Error(`${a[0]} exited ${srcCode}: ${Buffer.concat(srcErrChunks).toString().trim()}`))
      else if (dstCode !== 0) reject(dstErr ?? new Error(`${b[0]} exited ${dstCode}`))
      else resolve()
    }
    src.on('close', (code) => {
      srcCode = code ?? 1
      // src is done: end b's stdin so b can finish draining and exit.
      dst.stdin.end()
      settle()
    })
    dst.on('close', (code) => {
      dstCode = code ?? 1
      settle()
    })
  })
}

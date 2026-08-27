// `docker exec` into a MANAGED agent container — the shared wrapper for the
// two surfaces that reach an agent's own filesystem through its running
// container (agent-memory.ts reads and writes MEMORY.md; agent-crons.ts reads
// jobs.json and drives the `hermes cron` CLI). Both wrapped this by hand,
// byte-identical except the timeout, because going through the container —
// rather than keeping a Talaria-side copy — is the point: the agent curates
// its own state at runtime, and a second copy would drift inside a minute.
//
// Args never touch a shell (execFile, argv only — the cron prompts and the
// memory path are data, not command lines). On failure, stderr is the half a
// caller wants: it is what `docker exec` itself reported, and it beats the
// node error text when both exist. Output is captured whole up to 4MB, which
// jobs.json and MEMORY.md both sit far under.
import { execFile } from 'node:child_process'
import { managedContainer } from './fleet-docker'

/** Run `command` inside the agent's container. `-i` is added exactly when
 *  `input` is given (a write pipes its payload through stdin); the default
 *  timeout is the cron CLI's old bound, and a caller with a tighter one —
 *  memory's cat — says so. */
export function dockerExec(
  container: string,
  command: string[],
  opts: { timeoutMs?: number; input?: string } = {},
): Promise<{ stdout: string; stderr: string }> {
  return new Promise((res, rej) => {
    const args = ['exec', ...(opts.input !== undefined ? ['-i'] : []), container, ...command]
    const child = execFile('docker', args, { timeout: opts.timeoutMs ?? 30_000, maxBuffer: 4 * 1024 * 1024 }, (err, stdout, stderr) =>
      err ? rej(new Error(String(stderr).trim() || err.message)) : res({ stdout: String(stdout), stderr: String(stderr) }),
    )
    if (opts.input !== undefined) {
      child.stdin?.write(opts.input)
      child.stdin?.end()
    }
  })
}

/** The container a department's agent lives in. Slot-aware: a rolled agent
 *  lives in the '-b' service until its next roll, so the name is resolved per
 *  call, never cached — the roll can happen between any two commands. */
export const agentContainer = (department: string) => managedContainer(department)

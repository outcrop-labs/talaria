// The run watch carries two conversations in one tail. Hermes replies and
// its own tool calls are the agent. A terminal call that invokes the coding
// harness (opencode, Pi, Oh My Pi) is a turn: the steer the agent sent, the
// harness output, and the prose the agent wrote back before it did anything
// else. The panes share this split so a finished session and a live one
// agree.

export type WatchFrame = {
  t: string
  v: string
  s?: string
  p?: string
  r?: string
  ms?: number
}

export type HarnessTurn = {
  steer: string
  output: string
  response: string
  running: boolean
}

const HARNESS = /pi-coding-agent|oh-my-pi|\bopencode\b/

/** Tools that can RUN something. Naming the harness is not invoking it:
 *  `skill_view` of the oh-my-pi skill is documentation, `read_file` is
 *  reading, and `find -name '*oh-my-pi*'` is an agent hunting for a binary
 *  it was never handed a command for. All three used to open a "turn". */
const EXEC_TOOLS = new Set(['terminal', 'process_manage', 'execute_code'])

/** Every invoke template carries `--session-dir`; nothing that merely talks
 *  about the harness does. This is what separates running it from looking
 *  for it. */
const HARNESS_INVOKE = /--session-dir/

/** The managed-process handle a backgrounded harness is polled against. */
const SESSION_ID = /"session_id"\s*:\s*"([^"\s]{1,64})"/

/** An invocation of the coding harness, not a mention of it. */
export function isHarnessFrame(ev: WatchFrame): boolean {
  if (ev.t !== 'tool' && ev.t !== 'toolfull') return false
  if (!EXEC_TOOLS.has(ev.v)) return false
  const text = `${ev.p ?? ''}\n${ev.r ?? ''}`
  return HARNESS.test(text) && HARNESS_INVOKE.test(text)
}

/** The process session a frame is about, if it names one. */
export function sessionIdOf(ev: WatchFrame): string | null {
  return SESSION_ID.exec(`${ev.p ?? ''}\n${ev.r ?? ''}`)?.[1] ?? null
}

function frameEnded(ev: WatchFrame): boolean {
  if (ev.s === 'completed' || ev.s === 'error') return true
  // A managed process answers with its own status; "timeout" means the poll
  // gave up, not the process, so the turn is still open.
  return /"status"\s*:\s*"(completed|exited|done|error)"/.test(ev.r ?? '')
}

/** The command a preview describes. A backgrounded launch wraps it in the
 *  process tool's own JSON, and the wrapper's keys ("action", "command",
 *  "session_id") are quoted strings too — so reading the ask straight out of
 *  the preview picks up `command` rather than what was asked. */
function commandOf(preview: string): string | null {
  try {
    const parsed: unknown = JSON.parse(preview)
    const cmd = (parsed as { command?: unknown }).command
    return typeof cmd === 'string' ? cmd : null
  } catch {
    return null
  }
}

/** The ask inside a harness command. The invoke template quotes it last. */
export function harnessSteer(preview: string | undefined): string {
  if (!preview) return ''
  const cmd = commandOf(preview) ?? preview
  const quoted = [...cmd.matchAll(/"([^"\n]{1,2000})"/g)].map((m) => m[1] ?? '')
  const ask = quoted.filter((q) => !HARNESS.test(q) && !q.startsWith('-')).at(-1)
  return (ask ?? cmd).trim()
}

function sameSteer(a: string, b: string): boolean {
  if (!a || !b) return true
  return a === b || a.includes(b) || b.includes(a)
}

/** Completed work-session turns, in order, as the frames the watch stored. */
export function framesFromTranscript(body: string | null | undefined): WatchFrame[] {
  if (!body) return []
  const out: WatchFrame[] = []
  for (const raw of body.split('\n## Turn ')) {
    const n = raw.slice(0, raw.indexOf('\n')).trim()
    if (!/^\d+$/.test(n)) continue
    const streamAt = raw.indexOf('### Stream')
    const jsonBlock = streamAt >= 0 ? raw.slice(streamAt + 11).match(/```json\n([\s\S]*?)```/) : null
    for (const line of (jsonBlock?.[1] ?? '').split('\n')) {
      if (!line.startsWith('{')) continue
      try {
        out.push(JSON.parse(line) as WatchFrame)
      } catch {
        // a line we cannot parse is a line we skip
      }
    }
  }
  return out
}

export function splitWatch(frames: WatchFrame[]): { agent: WatchFrame[]; turns: HarnessTurn[] } {
  const agent: WatchFrame[] = []
  const turns: HarnessTurn[] = []
  let responding = false
  let prose = ''

  const flushProse = () => {
    const text = prose
    prose = ''
    if (!text) return
    if (responding && turns.length > 0) {
      turns[turns.length - 1]!.response += text
      return
    }
    agent.push({ t: 'd', v: text })
  }

  // A harness started in the background is polled through its managed
  // process handle, and those polls never name the binary. Without the
  // handle, every one of them reads as an ordinary agent tool call and its
  // output — the harness's whole side of the conversation — lands in the
  // agent pane as raw JSON instead of in the turn it belongs to.
  const harnessSessions = new Set<string>()

  for (const ev of frames) {
    if (ev.t === 'd') {
      prose += ev.v
      continue
    }
    if (isHarnessFrame(ev)) {
      flushProse()
      responding = false
      const sid = sessionIdOf(ev)
      if (sid) harnessSessions.add(sid)
      const steer = harnessSteer(ev.p)
      const open = turns.at(-1)
      const output = ev.r ?? ''
      const running = !output && !frameEnded(ev)
      if (open?.running && sameSteer(open.steer, steer)) {
        open.steer = open.steer || steer
        open.output = output || open.output
        open.running = running
      } else {
        turns.push({ steer, output, response: '', running })
      }
      if (!running) responding = true
      continue
    }
    // A poll against a session we know is the harness: its output is the
    // turn's output, however the agent happened to fetch it.
    const sid = sessionIdOf(ev)
    const open = turns.at(-1)
    if (sid && harnessSessions.has(sid) && open) {
      flushProse()
      responding = false
      if (ev.r) open.output += (open.output ? '\n' : '') + ev.r
      if (frameEnded(ev)) {
        open.running = false
        responding = true
      }
      continue
    }
    flushProse()
    responding = false
    agent.push(ev)
  }
  flushProse()
  return { agent, turns }
}

export function frameLabel(ev: WatchFrame): string {
  if (ev.t === 'tool') return `⚙ ${ev.v}${ev.s === 'running' ? ' …' : ev.s === 'completed' ? ' ✓' : ''}`
  if (ev.t === 'toolfull') return `⚙ ${ev.v}${ev.s === 'running' ? ' …' : ' ✓'}`
  if (ev.t === 'wtool') return `🛠 ${ev.v}${ev.ms ? ` (${(ev.ms / 1000).toFixed(1)}s)` : ''}`
  if (ev.t === 'r') return `· ${ev.v}`
  if (ev.t === 'err') return `⚠ ${ev.v}`
  return ev.v
}

/** What a tool call shows in the AGENT pane.
 *
 *  The full text stays in the transcript; this is the reading view. A
 *  managed-process poll answers with up to 2 KiB of JSON, and a session that
 *  drives a build answers with hundreds of them — which is what turned the
 *  agent pane into a wall of JSON with the actual replies lost inside it.
 */
export function toolDetail(ev: WatchFrame, limit = 240): string {
  const clip = (s: string) =>
    s.length > limit ? `${s.slice(0, limit).trimEnd()}… (${s.length} chars)` : s
  return [ev.p ? clip(ev.p) : '', ev.r ? `→ ${clip(ev.r)}` : ''].filter(Boolean).join('\n')
}

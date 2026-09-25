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

/** A terminal invocation of the coding harness, not a Talaria tool. */
export function isHarnessFrame(ev: WatchFrame): boolean {
  if (ev.t !== 'tool' && ev.t !== 'toolfull') return false
  return HARNESS.test(`${ev.v}\n${ev.p ?? ''}\n${ev.r ?? ''}`)
}

/** The ask inside a harness command. The invoke template quotes it last. */
export function harnessSteer(preview: string | undefined): string {
  if (!preview) return ''
  const quoted = [...preview.matchAll(/"([^"\n]{1,2000})"/g)].map((m) => m[1] ?? '')
  const ask = quoted.filter((q) => !HARNESS.test(q) && !q.startsWith('-')).at(-1)
  return (ask ?? preview).trim()
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

  for (const ev of frames) {
    if (ev.t === 'd') {
      prose += ev.v
      continue
    }
    if (isHarnessFrame(ev)) {
      flushProse()
      responding = false
      const steer = harnessSteer(ev.p)
      const open = turns.at(-1)
      const output = ev.r ?? ''
      const running = !output && ev.s !== 'completed' && ev.s !== 'error'
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

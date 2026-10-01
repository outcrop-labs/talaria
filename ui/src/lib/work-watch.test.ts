import { describe, expect, it } from 'vitest'
import {
  framesFromTranscript,
  isHarnessFrame,
  splitWatch,
  toolDetail,
  type WatchFrame,
} from '@/lib/work-watch'

const harnessCmd =
  'npx -y @oh-my-pi/pi-coding-agent@latest --mode json --auto-approve --session-dir /opt/data/workbench/sessions/job-1 --model talaria/code "add the missing test"'

// ── Shapes taken from a real run ───────────────────────────────────────────
// Every frame below is the shape production actually emits, read off the
// transcript of run 5a907c10 on 2026-09-30 (1897 frames). The previous
// version of this file invented a foreground `terminal` call carrying the
// whole harness command, which is a shape that run never produced once —
// and that is why a pane full of raw JSON, and a Turns tab made entirely of
// false positives, both passed a green test suite.
const realFrames = {
  // The agent READING the oh-my-pi skill. Documentation, not an invocation.
  skillView: { t: 'toolfull', v: 'skill_view', s: 'completed', p: '📚 oh-my-pi', r: 'Oh My Pi (omp) is…' },
  // The agent HUNTING for a binary, because it was never handed a command.
  findBinary: {
    t: 'toolfull',
    v: 'terminal',
    s: 'completed',
    p: "💻 find /opt/data/workbench/harness -maxdepth 3 \\( -name 'omp*' -o -name '*oh-my-pi*' \\) 2>/dev/null + 3 commands",
    r: '',
  },
  doctor: {
    t: 'toolfull',
    v: 'mcp__workbench__doctor',
    s: 'completed',
    p: '{}',
    r: '{"harness":{"slug":"oh-my-pi","probe":"npx -y @oh-my-pi/pi-coding-agent@latest --version"}}',
  },
  // A build the agent ran itself, polled through a managed process.
  cargoPoll: {
    t: 'toolfull',
    v: 'process_manage',
    s: 'completed',
    p: '{"action": "wait", "session_id": "proc_cargo0001", "timeout": 590}',
    r: '{"status": "timeout", "command": "cd /opt/data/workbench/jobs/2e5/repo/api && cargo test -p talaria-tasks"}',
  },
} satisfies Record<string, WatchFrame>

describe('isHarnessFrame', () => {
  it('does not mistake talking about the harness for running it', () => {
    // The whole Turns tab for run 5a907c10 was built out of these.
    expect(isHarnessFrame(realFrames.skillView)).toBe(false)
    expect(isHarnessFrame(realFrames.findBinary)).toBe(false)
    expect(isHarnessFrame(realFrames.doctor)).toBe(false)
    expect(isHarnessFrame({ t: 'toolfull', v: 'read_file', p: 'skills/oh-my-pi/SKILL.md', r: '…' })).toBe(false)
    expect(isHarnessFrame({ t: 'toolfull', v: 'skill_manage', p: 'oh-my-pi', r: 'ok' })).toBe(false)
  })

  it('recognises an actual invocation, foreground or backgrounded', () => {
    expect(isHarnessFrame({ t: 'tool', v: 'terminal', s: 'running', p: harnessCmd })).toBe(true)
    expect(
      isHarnessFrame({
        t: 'tool',
        v: 'process_manage',
        s: 'running',
        p: `{"action": "start", "session_id": "proc_harness01", "command": "${harnessCmd.replace(/"/g, '\\"')}"}`,
      }),
    ).toBe(true)
  })
})

describe('splitWatch', () => {
  it('keeps Hermes replies and tool calls off the harness turns', () => {
    const frames: WatchFrame[] = [
      { t: 'd', v: 'Reading the ticket. ' },
      { t: 'toolfull', v: 'get_ticket', s: 'completed', p: '{"id":"PLAT-1"}', r: 'title: fix the gate' },
      { t: 'd', v: 'Starting the harness.' },
      { t: 'tool', v: 'terminal', s: 'running', p: harnessCmd },
      { t: 'toolfull', v: 'terminal', s: 'completed', p: harnessCmd, r: '{"type":"agent_end","text":"test added"}' },
      { t: 'd', v: 'The harness added the test. I will ask it to run them.' },
      { t: 'wtool', v: 'comment', p: '{"body":"noted"}', r: 'ok' },
    ]
    const { agent, turns } = splitWatch(frames)
    expect(agent.map((f) => f.v)).toEqual([
      'Reading the ticket. ',
      'get_ticket',
      'Starting the harness.',
      'comment',
    ])
    expect(turns).toEqual([
      {
        steer: 'add the missing test',
        output: '{"type":"agent_end","text":"test added"}',
        response: 'The harness added the test. I will ask it to run them.',
        running: false,
      },
    ])
  })

  it('folds a backgrounded harness session into the turn it belongs to', () => {
    // The harness is started as a managed process and then POLLED. Those
    // polls never name the binary, so without the session handle each one
    // read as an ordinary agent tool call and dumped its JSON in the agent
    // pane — the harness's whole side of the conversation, in the wrong box.
    const start = `{"action": "start", "session_id": "proc_harness01", "command": "${harnessCmd.replace(/"/g, '\\"')}"}`
    const frames: WatchFrame[] = [
      { t: 'd', v: 'Driving the harness.' },
      { t: 'tool', v: 'process_manage', s: 'running', p: start },
      {
        t: 'toolfull',
        v: 'process_manage',
        s: 'completed',
        p: '{"action": "wait", "session_id": "proc_harness01", "timeout": 590}',
        r: '{"type":"message_end","text":"wrote the test"}',
      },
      {
        t: 'toolfull',
        v: 'process_manage',
        s: 'completed',
        p: '{"action": "wait", "session_id": "proc_harness01", "timeout": 60}',
        r: '{"status":"completed","type":"agent_end"}',
      },
      { t: 'd', v: 'It finished; reviewing the diff.' },
    ]
    const { agent, turns } = splitWatch(frames)
    // Not one harness poll in the agent pane.
    expect(agent.map((f) => f.v)).toEqual(['Driving the harness.'])
    expect(turns).toHaveLength(1)
    expect(turns[0]!.steer).toBe('add the missing test')
    expect(turns[0]!.output).toContain('wrote the test')
    expect(turns[0]!.output).toContain('agent_end')
    expect(turns[0]!.running).toBe(false)
    expect(turns[0]!.response).toBe('It finished; reviewing the diff.')
  })

  it("leaves the agent's own work in the agent pane and invents no turns", () => {
    // Exactly the run that prompted this: the harness is never invoked, so
    // there are NO turns — and the build the agent ran itself belongs to the
    // agent, not to a harness session it never opened.
    const { agent, turns } = splitWatch([
      { t: 'd', v: 'Looking for the harness. ' },
      realFrames.doctor,
      realFrames.skillView,
      realFrames.findBinary,
      realFrames.cargoPoll,
      { t: 'd', v: 'I will build it myself.' },
    ])
    expect(turns).toEqual([])
    expect(agent.map((f) => f.v)).toEqual([
      'Looking for the harness. ',
      'mcp__workbench__doctor',
      'skill_view',
      'terminal',
      'process_manage',
      'I will build it myself.',
    ])
  })

  it('reads harness frames back out of a retained transcript', () => {
    const body = [
      '',
      '## Turn 1',
      '',
      '### Prompt',
      '',
      '```',
      'work the ticket',
      '```',
      '',
      '### Stream',
      '',
      '```json',
      JSON.stringify({ t: 'd', v: 'On it.' }),
      JSON.stringify({ t: 'toolfull', v: 'terminal', s: 'completed', p: harnessCmd, r: 'done' }),
      JSON.stringify({ t: 'd', v: 'Landed.' }),
      '```',
    ].join('\n')
    const { agent, turns } = splitWatch(framesFromTranscript(body))
    expect(agent.map((f) => f.v)).toEqual(['On it.'])
    expect(turns[0]).toMatchObject({ steer: 'add the missing test', output: 'done', response: 'Landed.' })
  })
})

describe('toolDetail', () => {
  it('clips a result instead of pasting kilobytes of JSON into the pane', () => {
    const big = { t: 'toolfull', v: 'process_manage', p: '{"action":"wait"}', r: 'x'.repeat(2000) }
    const out = toolDetail(big)
    expect(out.length).toBeLessThan(600)
    expect(out).toContain('(2000 chars)')
    // Short detail is left exactly as it was.
    expect(toolDetail({ t: 'toolfull', v: 'comment', p: 'hi', r: 'ok' })).toBe('hi\n→ ok')
  })
})

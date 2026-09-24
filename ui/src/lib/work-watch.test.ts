import { describe, expect, it } from 'vitest'
import { framesFromTranscript, isHarnessFrame, splitWatch, type WatchFrame } from '@/lib/work-watch'

const harnessCmd =
  'npx -y @oh-my-pi/pi-coding-agent@latest --mode json --auto-approve --session-dir /opt/data/workbench/sessions/job-1 --model talaria/code "add the missing test"'

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
    expect(isHarnessFrame(frames[3]!)).toBe(true)
    expect(isHarnessFrame(frames[1]!)).toBe(false)
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

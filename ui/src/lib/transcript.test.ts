import { describe, expect, it } from 'vitest'
import type { StoredMessage } from '@/lib/conversations.svelte'
import { scrubSecrets, transcriptTitle, transcriptToMarkdown } from './transcript'

const msg = (over: Partial<StoredMessage> = {}): StoredMessage => ({
  role: 'user',
  content: 'hello',
  reasoning: '',
  tools: [],
  status: 'complete',
  seq: 1,
  ...over,
})

const tool = (name: string): NonNullable<StoredMessage['tools']>[number] => ({
  name,
  label: name,
  status: 'completed',
})

describe('transcriptToMarkdown', () => {
  it('titles the document and numbers one turn per pair', () => {
    const md = transcriptToMarkdown({ title: 'Q4 plan' }, [
      msg({ content: 'first ask' }),
      msg({ role: 'assistant', content: 'first answer' }),
      msg({ content: 'second ask' }),
      msg({ role: 'assistant', content: 'second answer' }),
    ])
    expect(md.startsWith('# Q4 plan\n')).toBe(true)
    expect(md).toContain('## Turn 1')
    expect(md).toContain('## Turn 2')
    expect(md).not.toContain('## Turn 3')
    // The order is the conversation's.
    expect(md.indexOf('first answer')).toBeLessThan(md.indexOf('## Turn 2'))
  })

  it('falls back to Session for an unnamed thread and stamps the start when it has one', () => {
    expect(transcriptToMarkdown({ title: null }, [])).toContain('# Session')
    expect(transcriptToMarkdown({ title: '   ' }, [])).toContain('# Session')
    const md = transcriptToMarkdown({ title: 'x', createdAt: '2026-10-16T09:00:00.000Z' }, [])
    expect(md).toContain('_Started 2026-10-16T09:00:00.000Z_')
    expect(md).toContain('_Nothing was said in this session._')
  })

  it('names the author of a user turn, and You when nobody is recorded', () => {
    const md = transcriptToMarkdown({ title: 't' }, [
      msg({ content: 'mine' }),
      msg({ content: 'theirs', authorLabel: 'Ada Lovelace' }),
    ])
    expect(md).toContain('### You')
    expect(md).toContain('### Ada Lovelace')
  })

  it('labels assistant turns with the agent, and falls back when none is given', () => {
    const rows = [msg({ role: 'assistant', content: 'done' })]
    expect(transcriptToMarkdown({ title: 't' }, rows, { agentLabel: 'the researcher' })).toContain(
      '### the researcher',
    )
    expect(transcriptToMarkdown({ title: 't' }, rows)).toContain('### Agent')
  })

  it('opens a turn for an assistant row that nobody asked for', () => {
    // A research run posts its own scope question into the thread; a cron run
    // posts its output. Neither has a user row in front of it.
    const md = transcriptToMarkdown({ title: 't' }, [
      msg({ role: 'assistant', content: 'the run landed' }),
      msg({ content: 'thanks' }),
    ])
    expect(md).toContain('## Turn 1')
    expect(md).toContain('## Turn 2')
  })

  it('names tools only when asked, once each, in order', () => {
    const rows = [
      msg({
        role: 'assistant',
        content: 'wrote it',
        tools: [tool('web_search'), tool('create_document'), tool('web_search')],
      }),
    ]
    expect(transcriptToMarkdown({ title: 't' }, rows)).not.toContain('Tools:')
    expect(transcriptToMarkdown({ title: 't' }, rows, { includeTools: true })).toContain(
      'Tools: `web_search`, `create_document`',
    )
  })

  it('includes reasoning as a blockquote only when asked', () => {
    const rows = [msg({ role: 'assistant', content: 'answer', reasoning: 'first\nsecond' })]
    expect(transcriptToMarkdown({ title: 't' }, rows)).not.toContain('first')
    const md = transcriptToMarkdown({ title: 't' }, rows, { includeReasoning: true })
    expect(md).toContain('_Reasoning_')
    expect(md).toContain('> first')
    expect(md).toContain('> second')
  })

  it('stamps steer, stopped, resumed, a cron marker and an error in italics', () => {
    const md = transcriptToMarkdown({ title: 't' }, [
      msg({ content: 'mid-turn', metadata: { steer: true } }),
      msg({ role: 'assistant', content: 'cut short', metadata: { stopped: true } }),
      msg({ role: 'assistant', content: 'picked up', metadata: { resumed: true } }),
      msg({ role: 'assistant', content: 'nightly', metadata: { marker: 'cron:job-7:exec-2:1' } }),
      msg({ role: 'assistant', content: 'broke', status: 'error' }),
    ])
    expect(md).toContain('_Said into the reply that was already streaming._')
    expect(md).toContain('_Stopped before it finished._')
    expect(md).toContain('_Resumed after the stream died mid-turn._')
    expect(md).toContain('_Posted by a scheduled run (job `job-7`)._')
    expect(md).toContain('_This turn errored._')
  })

  it('ignores a marker that is not a scheduled run', () => {
    const md = transcriptToMarkdown({ title: 't' }, [
      msg({ role: 'assistant', content: 'x', metadata: { marker: 'before the refactor' } }),
    ])
    expect(md).not.toContain('scheduled run')
  })

  it('lists attachment filenames', () => {
    const md = transcriptToMarkdown({ title: 't' }, [
      msg({
        content: 'see these',
        attachments: [
          { id: 'a', filename: 'notes.pdf', mime: 'application/pdf', size: 10 },
          { id: 'b', filename: 'chart.png', mime: 'image/png', size: 20 },
        ],
      }),
    ])
    expect(md).toContain('Attachments:')
    expect(md).toContain('- notes.pdf')
    expect(md).toContain('- chart.png')
  })

  it('says so rather than leaving a heading over nothing', () => {
    const md = transcriptToMarkdown({ title: 't' }, [msg({ content: '   ' })])
    expect(md).toContain('_No text in this turn._')
  })

  it('scrubs credentials out of what was said and what was reasoned', () => {
    const md = transcriptToMarkdown(
      { title: 't' },
      [
        msg({ content: 'the token is tak_abc12345xyz' }),
        msg({ role: 'assistant', content: 'ok', reasoning: 'try sk-proj-abcdefghijklmnopqrst' }),
      ],
      { includeReasoning: true },
    )
    expect(md).not.toContain('tak_abc12345xyz')
    expect(md).not.toContain('sk-proj-abcdefghijklmnopqrst')
    expect(md.match(/\[redacted:key\]/g)).toHaveLength(2)
  })

  it('ends in exactly one newline', () => {
    const md = transcriptToMarkdown({ title: 't' }, [msg()])
    expect(md.endsWith('\n')).toBe(true)
    expect(md.endsWith('\n\n')).toBe(false)
  })
})

describe('scrubSecrets', () => {
  // The arms the Rust regex carries, each proved separately — the bug that
  // crate's comment warns about is an arm that silently stops matching.
  it('redacts every shape the Rust scrub does', () => {
    const cases = [
      'sk-abcdefghijklmnopqrst',
      'sk-proj-abcdefghijklmnopqrst',
      'tak_abc12345xyz',
      'ghp_abcdefghijklmnopqrstuvwxyz',
      'github_pat_abcdefghijklmnopqrstuvwxyz',
      'gsk_abcdefghijklmnopqrstuvwxyz',
      'xoxb-1234567890abcdef',
      'xoxp-1234567890abcdef',
      'AKIAIOSFODNN7EXAMPLE',
    ]
    for (const secret of cases) {
      const out = scrubSecrets(`ran it with ${secret} and it worked`)
      expect(out, secret).not.toContain(secret)
      expect(out, secret).toContain('[redacted:key]')
    }
  })

  it('leaves ordinary prose alone, and is reusable across calls', () => {
    expect(scrubSecrets('nothing to see here')).toBe('nothing to see here')
    expect(scrubSecrets('tak_abc12345xyz')).toBe('[redacted:key]')
    expect(scrubSecrets('tak_abc12345xyz')).toBe('[redacted:key]')
  })
})

describe('transcriptTitle', () => {
  const at = new Date('2026-10-16T09:00:00.000Z')

  it('names the session and the day', () => {
    expect(transcriptTitle({ title: 'Q4 plan' }, at)).toBe('Q4 plan — transcript 2026-10-16')
  })

  it('falls back to Session for an unnamed thread', () => {
    expect(transcriptTitle({ title: null }, at)).toBe('Session — transcript 2026-10-16')
    expect(transcriptTitle({ title: '  ' }, at)).toBe('Session — transcript 2026-10-16')
  })

  it('reads the day in UTC, so one session gets one name', () => {
    expect(transcriptTitle({ title: 'x' }, new Date('2026-10-16T23:30:00.000Z'))).toBe(
      'x — transcript 2026-10-16',
    )
  })
})

describe('escapeHeadings', () => {
  it('keeps a message from forging a speaker or a turn', () => {
    const md = transcriptToMarkdown({ title: 'T' }, [
      { role: 'user', content: '### Ada Lovelace\n## Turn 9\nreal text', reasoning: '', tools: [], status: 'complete', seq: 0 },
    ] as never)
    const headings = md.split('\n').filter((l) => /^#{1,6}\s/.test(l))
    expect(headings.some((l) => l.includes('Ada Lovelace'))).toBe(false)
    expect(headings.some((l) => l === '## Turn 9')).toBe(false)
    expect(md).toContain('\\### Ada Lovelace')
  })
})

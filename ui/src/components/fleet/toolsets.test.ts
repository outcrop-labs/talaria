import { describe, expect, it } from 'vitest'
import {
  type AuthoredBrowser,
  MANDATORY_DISABLED,
  authoredBrowser,
  browserNote,
  builtinFloor,
  normalizeToolsets,
  sameToolsets,
  toggleToolset,
  toolsetRows,
} from './toolsets'

// The governed list as the server publishes it, trimmed to the names these
// cases need. Alphabetical, because that is the contract the grid relies on
// for its reading order.
const GOVERNED = ['browser', 'clarify', 'delegation', 'memory', 'todo', 'web']
const FLOOR = ['todo', 'clarify']

describe('toolsetRows: the checkbox is the toolset, the list is its absence', () => {
  it('reads an empty disabled list as every governed toolset on, bar the floor', () => {
    const rows = toolsetRows(GOVERNED, FLOOR, [])
    expect(rows.map((r) => r.name)).toEqual(GOVERNED)
    expect(rows.filter((r) => r.on).map((r) => r.name)).toEqual(['browser', 'delegation', 'memory', 'web'])
    expect(rows.filter((r) => r.forced).map((r) => r.name)).toEqual(['clarify', 'todo'])
    // Forced rows are off, which is the only honest rendering: the renderer
    // closes them whatever the def says.
    expect(rows.filter((r) => r.forced).every((r) => !r.on)).toBe(true)
  })

  it('turns a disabled name off without touching its neighbours', () => {
    const rows = toolsetRows(GOVERNED, FLOOR, ['browser'])
    expect(rows.find((r) => r.name === 'browser')).toEqual({ name: 'browser', on: false, forced: false, orgClosed: false })
    expect(rows.find((r) => r.name === 'web')?.on).toBe(true)
  })

  it('renders a legacy def that stored a forced name identically to one that did not', () => {
    // Same rendered config, so the same grid — otherwise an operator reads a
    // difference that does not exist downstream.
    expect(toolsetRows(GOVERNED, FLOOR, ['todo', 'browser'])).toEqual(toolsetRows(GOVERNED, FLOOR, ['browser']))
  })

  it('dedupes and trims the governed list, and drops a blank name', () => {
    const rows = toolsetRows([' web ', 'web', '', 'browser'], FLOOR, [])
    expect(rows.map((r) => r.name)).toEqual(['web', 'browser'])
  })

  it('matches a disabled name that arrived with whitespace', () => {
    expect(toolsetRows(GOVERNED, FLOOR, [' browser '])?.find((r) => r.name === 'browser')?.on).toBe(false)
  })

  it('forces nothing when the server publishes no floor', () => {
    const rows = toolsetRows(GOVERNED, [], [])
    expect(rows.some((r) => r.forced)).toBe(false)
    expect(rows.every((r) => r.on)).toBe(true)
  })

  it('locks an org-closed name with its own reason, whether or not the floor repeats it', () => {
    // The server's floor carries the org part inside it…
    const inside = toolsetRows(GOVERNED, [...FLOOR, 'browser'], [], ['browser'])
    expect(inside.find((r) => r.name === 'browser')).toEqual({ name: 'browser', on: false, forced: true, orgClosed: true })
    // …and the pair is forced for Talaria's reason, not the admin's.
    expect(inside.find((r) => r.name === 'todo')).toEqual({ name: 'todo', on: false, forced: true, orgClosed: false })
    // A floor that did not repeat the org name locks it all the same.
    expect(toolsetRows(GOVERNED, FLOOR, [], ['browser']).find((r) => r.name === 'browser')?.forced).toBe(true)
    // A def that closed it itself renders the same as one that did not: the
    // rendered config is identical while the floor stands.
    expect(toolsetRows(GOVERNED, FLOOR, ['browser'], ['browser'])).toEqual(toolsetRows(GOVERNED, FLOOR, [], ['browser']))
  })
})

describe('builtinFloor: what is never stored', () => {
  it('is the pair with the org part taken out', () => {
    expect(builtinFloor({ mandatoryDisabled: ['todo', 'clarify', 'terminal'], orgDisabled: ['terminal'] })).toEqual(['todo', 'clarify'])
  })

  it('is the whole floor on an api that publishes no org part, and the pair with no vocabulary at all', () => {
    expect(builtinFloor({ mandatoryDisabled: ['todo', 'clarify'] })).toEqual(['todo', 'clarify'])
    expect(builtinFloor(undefined)).toEqual(MANDATORY_DISABLED)
  })

  it('keeps an org-closed name in the stored list, so lifting the floor restores what a manager chose', () => {
    const builtin = builtinFloor({ mandatoryDisabled: ['todo', 'clarify', 'terminal'], orgDisabled: ['terminal'] })
    // A manager closed terminal before the admin did; an unrelated Apply
    // must not rewrite the list without it.
    expect(toggleToolset(['terminal'], 'browser', false, builtin)).toEqual(['browser', 'terminal'])
    expect(normalizeToolsets(['terminal', 'todo'], builtin)).toEqual(['terminal'])
  })
})

describe('toggleToolset: the whole list, sorted and deduped', () => {
  it('turning a toolset OFF adds it to the disabled list', () => {
    expect(toggleToolset([], 'browser', false, FLOOR)).toEqual(['browser'])
  })

  it('turning a toolset ON removes it', () => {
    expect(toggleToolset(['browser', 'web'], 'browser', true, FLOOR)).toEqual(['web'])
  })

  it('sorts and dedupes, so two routes to one set produce one list', () => {
    const a = toggleToolset(toggleToolset([], 'web', false, FLOOR), 'browser', false, FLOOR)
    const b = toggleToolset(toggleToolset([], 'browser', false, FLOOR), 'web', false, FLOOR)
    expect(a).toEqual(['browser', 'web'])
    expect(a).toEqual(b)
    expect(toggleToolset(['web', 'web', ' web '], 'browser', false, FLOOR)).toEqual(['browser', 'web'])
  })

  it('never stores a forced name, whichever way it is flipped', () => {
    expect(toggleToolset([], 'todo', false, FLOOR)).toEqual([])
    expect(toggleToolset(['todo', 'clarify', 'web'], 'browser', false, FLOOR)).toEqual(['browser', 'web'])
    expect(toggleToolset(['todo'], 'todo', true, FLOOR)).toEqual([])
  })

  it('still refuses the floor when the caller passes no forced list', () => {
    // The three-argument call is the contract other code may reach for; it
    // must not be the door a mandatory name walks through.
    expect(toggleToolset([], 'todo', false)).toEqual([])
    expect(toggleToolset([], 'clarify', false)).toEqual([])
    // The server constant's own order (talaria-agent-defs::MANDATORY_DISABLED).
    expect(MANDATORY_DISABLED).toEqual(['todo', 'clarify'])
  })

  it('leaves the caller’s list alone', () => {
    const before = ['web']
    toggleToolset(before, 'browser', false, FLOOR)
    expect(before).toEqual(['web'])
  })
})

describe('sameToolsets: order is not a change', () => {
  it('compares as sets, so the server order and the sorted order agree', () => {
    expect(sameToolsets(['web', 'browser'], ['browser', 'web'])).toBe(true)
    expect(sameToolsets([], [])).toBe(true)
  })

  it('sees a real difference', () => {
    expect(sameToolsets(['web'], ['web', 'browser'])).toBe(false)
    expect(sameToolsets(['web'], ['browser'])).toBe(false)
  })

  it('does not call a duplicate a difference', () => {
    expect(sameToolsets(['web', 'web'], ['web'])).toBe(true)
  })
})

describe('normalizeToolsets: the storable spelling', () => {
  it('sorts, trims, dedupes and drops the floor', () => {
    expect(normalizeToolsets([' web ', 'browser', 'web', 'todo', '', 'clarify'], FLOOR)).toEqual(['browser', 'web'])
  })

  it('reads a legacy def carrying a forced name as the same list without it', () => {
    // Which is what makes the Apply button honest: the rendered config for
    // these two is identical, so the grid must not offer a roll between them.
    expect(normalizeToolsets(['todo', 'browser'], FLOOR)).toEqual(normalizeToolsets(['browser'], FLOOR))
  })

  it('drops the floor with no forced list passed', () => {
    expect(normalizeToolsets(['todo', 'clarify', 'web'])).toEqual(['web'])
  })

  it('leaves the caller\u2019s list alone', () => {
    const before = ['web', 'browser']
    normalizeToolsets(before, FLOOR)
    expect(before).toEqual(['web', 'browser'])
  })
})

describe('browserNote: the terminal decides whether this agent has a browser', () => {
  const WITH_TERMINAL = [...GOVERNED, 'terminal'].sort()
  const closed = toolsetRows(WITH_TERMINAL, FLOOR, ['terminal'])
  const open = toolsetRows(WITH_TERMINAL, FLOOR, [])
  const NONE = /^Browser: none while the terminal is closed\. /

  it('says none, and why, while the terminal is closed', () => {
    const note = browserNote(closed)
    expect(note).toMatch(NONE)
    expect(note).toContain('skips its private-address check')
    expect(note).toContain('this machine, other agents and your LAN')
    expect(note).toContain('only with the terminal open')
    // No browser to name, and no engine to install.
    expect(note).not.toContain('browser_navigate')
    expect(note).not.toContain('browser_exec')
    expect(note).not.toContain('npm')
  })

  it('says none for a terminal closed by the org floor too', () => {
    const floored = toolsetRows(WITH_TERMINAL, [...FLOOR, 'terminal'], [], ['terminal'])
    expect(browserNote(floored)).toMatch(NONE)
  })

  it('names browser_exec and its engine while the terminal is open', () => {
    const note = browserNote(open)
    expect(note).toBe(
      'Browser: Hermes’ browser_exec, which comes with the terminal. Its engine is installed from npm onto the agent’s volume before the agent starts; with no route to the registry, the agent starts without browser tools.',
    )
  })

  it('reads a vocabulary with no terminal row as a closed terminal', () => {
    expect(browserNote(toolsetRows(GOVERNED, FLOOR, []))).toMatch(NONE)
  })

  const LOCAL = { kind: 'local', via: 'browser.backend: off' } as const
  const EXEC = { kind: 'exec', via: 'browser.backend: browser-use' } as const
  const CDP = { kind: 'cdp', via: 'browser.cdp_url' } as const
  const HOSTED = { kind: 'hosted', via: 'browser.cloud_provider: browserbase' } as const

  it('says nothing when the browser is off, closed org-wide, or absent', () => {
    for (const authored of [null, LOCAL, EXEC, CDP, HOSTED]) {
      expect(browserNote(toolsetRows(WITH_TERMINAL, FLOOR, ['browser']), authored)).toBeNull()
      expect(browserNote(toolsetRows(WITH_TERMINAL, FLOOR, ['browser', 'terminal']), authored)).toBeNull()
      expect(browserNote(toolsetRows(WITH_TERMINAL, [...FLOOR, 'browser'], [], ['browser']), authored)).toBeNull()
    }
    expect(browserNote(toolsetRows(['memory', 'web'], FLOOR, []))).toBeNull()
  })

  it('names what a browser the def authored gives with the terminal open', () => {
    // Anything but browser-use is the built-in tools, wherever they run.
    for (const authored of [LOCAL, { kind: 'local', via: 'browser.cloud_provider: camofox' } as const, CDP, HOSTED]) {
      const note = browserNote(open, authored)
      expect(note).toContain('the built-in browser tools (browser_navigate and the rest)')
      expect(note).toContain(`as this agent’s config sets ${authored.via}.`)
      expect(note).toContain('installed from npm')
    }
    const exec = browserNote(open, EXEC)
    expect(exec).toContain('Browser: Hermes’ browser_exec, which comes with the terminal')
    expect(exec).toContain('(this agent’s config sets browser.backend: browser-use)')
    expect(exec).toContain('installed from npm')
  })

  it('says a local browser the def authored is closed with the terminal', () => {
    // browser-use is browser_exec, which needs the terminal anyway: plain none.
    expect(browserNote(closed, EXEC)).toBe(browserNote(closed))
    // A local browser skips the check, so the renderer closes the toolset
    // (fleet_browser_local_close): still none, and the line says so.
    const off = browserNote(closed, LOCAL)
    expect(off).toMatch(NONE)
    expect(off).toContain('skips its private-address check')
    expect(off).toContain(
      'This agent’s config picks a browser Hermes runs locally (browser.backend: off), so Talaria closes the browser toolset for it until the terminal opens.',
    )
    expect(off).not.toContain('npm')
  })

  it('names a remote browser that needs no terminal', () => {
    const cdp = browserNote(closed, CDP)
    expect(cdp).not.toMatch(NONE)
    expect(cdp).toBe(
      'Browser: the built-in browser tools on the CDP endpoint this agent’s config sets (browser.cdp_url). Hermes keeps its private-address check for a remote browser, so it does not need the terminal.',
    )
    const hosted = browserNote(closed, HOSTED)
    expect(hosted).not.toMatch(NONE)
    expect(hosted).toContain('(browser.cloud_provider: browserbase), which keeps Hermes’ private-address check')
    expect(hosted).toContain('installs only while the terminal is open')
  })
})

describe('authoredBrowser: the def’s own browser, as Hermes reads it', () => {
  const cfg = (browser: unknown) => ({ raw: { browser } })

  it('reads a local browser', () => {
    expect(authoredBrowser(cfg({ backend: 'off' }))).toEqual({ kind: 'local', via: 'browser.backend: off' })
    expect(authoredBrowser(cfg({ backend: false }))).toEqual({ kind: 'local', via: 'browser.backend: off' })
    expect(authoredBrowser(cfg({ backend: ' Camofox ' }))).toEqual({ kind: 'local', via: 'browser.backend: camofox' })
    expect(authoredBrowser(cfg({ backend: 'off', cloud_provider: 'local', cdp_url: ' ' }))).toEqual({
      kind: 'local',
      via: 'browser.backend: off',
    })
    // Camofox overrides Browser Use mode, and Hermes calls it local.
    for (const backend of [undefined, 'browser-use']) {
      expect(authoredBrowser(cfg({ backend, cloud_provider: 'CAMOFOX' }))).toEqual({
        kind: 'local',
        via: 'browser.cloud_provider: camofox',
      })
    }
  })

  it('reads a remote browser, and the default spelled out', () => {
    expect(authoredBrowser(cfg({ backend: 'off', cdp_url: 'http://chrome:9222' }))).toEqual({ kind: 'cdp', via: 'browser.cdp_url' })
    expect(
      authoredBrowser(cfg({ backend: 'off', cloud_provider: 'Browserbase', auto_local_for_private_urls: false })),
    ).toEqual({
      kind: 'hosted',
      via: 'browser.cloud_provider: browserbase',
    })
    expect(authoredBrowser(cfg({ backend: ' Browser-Use ' }))).toEqual({
      kind: 'exec',
      via: 'browser.backend: browser-use',
    })
  })

  it('reads a browser Hermes lets reach private addresses as local, as the renderer does', () => {
    const SIDECAR =
      'Hermes sends its private addresses to a local Chromium while browser.auto_local_for_private_urls is on, as it is by default'
    // A hosted provider's sidecar: on when absent, and for anything
    // Python's bool() reads as true, the string "false" included.
    for (const provider of ['browserbase', 'Browser-Use', 'nous', 'firecrawl', 'a-plugin']) {
      const via = `browser.cloud_provider: ${provider.toLowerCase()}`
      for (const sidecar of [undefined, true, 'false', 'no', 1, ['x']]) {
        const browser: Record<string, unknown> = { backend: 'off', cloud_provider: provider }
        if (sidecar !== undefined) browser.auto_local_for_private_urls = sidecar
        expect(authoredBrowser(cfg(browser))).toEqual({ kind: 'local', via, unchecked: SIDECAR })
      }
      for (const off of [false, null, 0, '', [], {}]) {
        expect(
          authoredBrowser(cfg({ backend: 'off', cloud_provider: provider, auto_local_for_private_urls: off })),
        ).toEqual({ kind: 'hosted', via })
      }
    }
    // The check turned off, under either key, for each browser that would
    // otherwise keep it.
    const kept: [Record<string, unknown>, Record<string, unknown>, string[], AuthoredBrowser][] = [
      [{ backend: 'off', cdp_url: 'http://chrome:9222' }, {}, [], { kind: 'cdp', via: 'browser.cdp_url' }],
      [{ backend: 'off' }, {}, ['BROWSER_CDP_URL'], { kind: 'cdp', via: 'a BROWSER_CDP_URL secret' }],
      [
        { backend: 'off', cloud_provider: 'browserbase', auto_local_for_private_urls: false },
        {},
        [],
        { kind: 'hosted', via: 'browser.cloud_provider: browserbase' },
      ],
      [
        { backend: 'off' },
        { terminal: { backend: 'docker' } },
        [],
        { kind: 'checked', via: 'browser.backend: off', terminal: 'terminal.backend: docker' },
      ],
    ]
    for (const [browser, extra, secrets, want] of kept) {
      expect(authoredBrowser({ raw: { browser, ...extra } }, secrets)).toEqual(want)
      for (const on of [true, 'true', ' Yes ', 'ON', '1', 1]) {
        expect(authoredBrowser({ raw: { browser: { ...browser, allow_private_urls: on }, ...extra } }, secrets)).toEqual({
          kind: 'local',
          via: want.via,
          unchecked: 'browser.allow_private_urls',
        })
        expect(
          authoredBrowser({ raw: { browser, ...extra, security: { allow_private_urls: on } } }, secrets),
        ).toEqual({ kind: 'local', via: want.via, unchecked: 'security.allow_private_urls' })
      }
      // Falsy as is_truthy_value reads it, or not in a mapping.
      for (const off of [false, null, 'false', 'off', '2', 0]) {
        expect(authoredBrowser({ raw: { browser: { ...browser, allow_private_urls: off }, ...extra } }, secrets)).toEqual(want)
        expect(authoredBrowser({ raw: { browser, ...extra, security: { allow_private_urls: off } } }, secrets)).toEqual(want)
      }
      expect(authoredBrowser({ raw: { browser, ...extra, security: ['allow_private_urls'] } }, secrets)).toEqual(want)
    }
    // Hermes' default stays the default; Camofox and a plain local backend
    // are local already.
    expect(authoredBrowser(cfg({ allow_private_urls: true }))).toBeNull()
    expect(authoredBrowser(cfg({ cloud_provider: 'camofox', allow_private_urls: true }))).toEqual({
      kind: 'local',
      via: 'browser.cloud_provider: camofox',
    })
    // The grid line, with the terminal closed.
    const terminalClosed = toolsetRows([...GOVERNED, 'terminal'].sort(), FLOOR, ['terminal'])
    expect(browserNote(terminalClosed, authoredBrowser(cfg({ backend: 'off', cloud_provider: 'browserbase' })))).toBe(
      `Browser: none while the terminal is closed. On this image, Hermes’ own browser skips its private-address check, on the grounds that the terminal already reaches this machine, other agents and your LAN. That holds only with a terminal, so Talaria gives an agent the browser only with the terminal open. This agent’s config sets a browser (browser.cloud_provider: browserbase) that Hermes lets reach private addresses (${SIDECAR}), so Talaria closes the browser toolset for it until the terminal opens.`,
    )
    const cdpAllowed = authoredBrowser(cfg({ backend: 'off', cdp_url: 'http://chrome:9222', allow_private_urls: true }))
    expect(browserNote(terminalClosed, cdpAllowed)).toContain(
      'This agent’s config sets a browser (browser.cdp_url) that Hermes lets reach private addresses (browser.allow_private_urls), so Talaria closes the browser toolset for it until the terminal opens.',
    )
  })

  it('reads a BROWSER_CDP_URL secret as the CDP endpoint, as the renderer does', () => {
    const viaSecret = { kind: 'cdp', via: 'a BROWSER_CDP_URL secret' }
    expect(authoredBrowser(cfg({ backend: 'off' }), ['FIGMA_TOKEN', 'BROWSER_CDP_URL'])).toEqual(viaSecret)
    expect(authoredBrowser(cfg({ backend: false }), ['BROWSER_CDP_URL'])).toEqual(viaSecret)
    expect(authoredBrowser(cfg({ backend: ' Camofox ' }), ['BROWSER_CDP_URL'])).toEqual(viaSecret)
    // The env name wins over the config key (`_get_cdp_override_raw`).
    expect(authoredBrowser(cfg({ backend: 'off', cdp_url: 'http://chrome:9222' }), ['BROWSER_CDP_URL'])).toEqual(viaSecret)
    // It turns a Camofox selection off, which with no backend is Browser Use mode.
    expect(authoredBrowser(cfg({ cloud_provider: 'camofox' }), ['BROWSER_CDP_URL'])).toBeNull()
    // A def with no browser block stays on Hermes' default.
    expect(authoredBrowser({ raw: {} }, ['BROWSER_CDP_URL'])).toBeNull()
    // Other names choose nothing: the local browser stays local.
    expect(authoredBrowser(cfg({ backend: 'off' }), ['BROWSERBASE_BASE_URL', 'FIGMA_TOKEN'])).toEqual({
      kind: 'local',
      via: 'browser.backend: off',
    })
    const terminalClosed = toolsetRows([...GOVERNED, 'terminal'].sort(), FLOOR, ['terminal'])
    expect(browserNote(terminalClosed, viaSecret as AuthoredBrowser)).toBe(
      'Browser: the built-in browser tools on the CDP endpoint this agent’s secrets supply (a BROWSER_CDP_URL secret). Hermes keeps its private-address check for a remote browser, so it does not need the terminal.',
    )
  })

  it('reads a terminal that runs elsewhere as keeping Hermes’ check, as the renderer does', () => {
    const withTerminal = (browser: unknown, terminal: unknown) => ({ raw: { browser, terminal } })
    // Every backend v0.22.4 ships but local, in any case, and every value
    // that is not a string, as Hermes' str() spells it.
    for (const [backend, spelled] of [
      ['docker', 'docker'],
      ['singularity', 'singularity'],
      ['modal', 'modal'],
      ['daytona', 'daytona'],
      ['vercel_sandbox', 'vercel_sandbox'],
      ['ssh', 'ssh'],
      [' Docker ', 'docker'],
      [null, 'None'],
      [false, 'False'],
      [true, 'True'],
      [0, '0'],
    ] as const) {
      expect(authoredBrowser(withTerminal({ backend: 'off' }, { backend }))).toEqual({
        kind: 'checked',
        via: 'browser.backend: off',
        terminal: `terminal.backend: ${spelled}`,
      })
      // Camofox is asked first and stays local; a CDP endpoint stays a CDP endpoint.
      expect(authoredBrowser(withTerminal({ cloud_provider: 'camofox' }, { backend }))?.kind).toBe('local')
      expect(authoredBrowser(withTerminal({ backend: 'off' }, { backend }), ['BROWSER_CDP_URL'])?.kind).toBe('cdp')
    }
    // Local, as Hermes reads it: no map, no backend, or local or empty.
    for (const terminal of [undefined, null, 'docker', {}, { env_passthrough: ['A'] }, { backend: 'local' }, { backend: ' LOCAL ' }, { backend: '' }]) {
      expect(authoredBrowser(withTerminal({ backend: 'off' }, terminal))).toEqual({ kind: 'local', via: 'browser.backend: off' })
    }
    const checked = authoredBrowser(withTerminal({ backend: 'off' }, { backend: 'docker' }))
    const terminalClosed = toolsetRows([...GOVERNED, 'terminal'].sort(), FLOOR, ['terminal'])
    expect(browserNote(terminalClosed, checked)).toBe(
      'Browser: the built-in browser tools this agent’s config sets (browser.backend: off), which keep Hermes’ private-address check because its terminal runs elsewhere (terminal.backend: docker). They need the browser engine, which Talaria installs only while the terminal is open.',
    )
  })

  it('reads what leaves Hermes on its default, and every missing level, as null', () => {
    for (const backend of [null, undefined, '', '   ', true, 0, [], {}]) {
      expect(authoredBrowser(cfg({ backend }))).toBeNull()
    }
    // A CDP endpoint turns Camofox off, and with no backend that is Browser Use mode.
    expect(authoredBrowser(cfg({ cloud_provider: 'camofox', cdp_url: 'ws://chrome:9222' }))).toBeNull()
    expect(authoredBrowser(cfg({ cloud_provider: 'browserbase' }))).toBeNull()
    for (const config of [null, undefined, {}, { raw: null }, { raw: {} }, cfg(false), cfg('x'), cfg(['off'])]) {
      expect(authoredBrowser(config)).toBeNull()
    }
  })
})

// GitHub's "Latest" release is the desktop updater feed, and nothing else.
//
// The shell's updater reads one URL: /releases/latest/download/latest.json
// (desktop/src-tauri/tauri.conf.json, plugins.updater.endpoints). GitHub
// resolves /releases/latest to whichever release is marked Latest, and a new
// non-prerelease release takes that mark BY DEFAULT unless its creator says
// otherwise. So the flag is not cosmetic here:
//   - release.yml opens a vX.Y.Z (server) release that carries no latest.json
//     at all: desktop installers ship only on desktop-v releases. Made Latest,
//     it 404s every installed app's update check. It must say --latest=false,
//     on the create and on a re-run's edit.
//   - desktop-package.yml mints the desktop-vX.Y.0 release that IS the feed.
//     It is created --latest=false too (no latest.json on it yet), and marked
//     Latest with an explicit `gh release edit --latest` only after its
//     latest.json is uploaded, so the feed never points at a release without
//     one. The flip runs only on the mint path: `attach` also runs for a
//     dispatch that re-attaches into an existing release, and an unguarded
//     flip would make that release Latest. So the flip must sit in the `then`
//     branch of a shell `if` on "$MINT" = true (or under a step `if:` on
//     needs.resolve.outputs.mint), with MINT bound to that output in the step.
//   - any other workflow that creates a release states its intent (a
//     prerelease, which can never be Latest, or an explicit flag) and never
//     marks one Latest.
// check-invariants.mjs runs checkReleaseLatest over .github/workflows and the
// tauri config, and selfTest() first, so the check cannot go blind quietly.
// No imports: plain string work, runnable with no install like the rest of scripts/.

export const FEED_SUFFIX = '/releases/latest/download/latest.json'
const SERVER = 'release.yml'
const DESKTOP = 'desktop-package.yml'

/** Every `gh release create|edit` in a workflow's text, with its shell
 *  continuations joined, and the 1-based line it starts on. Comment lines are
 *  skipped, so a command quoted in a comment is not one. */
export function releaseCommands(text) {
  const lines = text.split('\n')
  const out = []
  for (let i = 0; i < lines.length; i++) {
    if (/^\s*#/.test(lines[i])) continue
    const m = /\bgh release (create|edit)\b/.exec(lines[i])
    if (!m) continue
    let cmd = lines[i].slice(m.index)
    let j = i
    while (/\\\s*$/.test(lines[j]) && j + 1 < lines.length) {
      cmd = cmd.replace(/\\\s*$/, ' ') + lines[++j].trim()
    }
    out.push({ line: i + 1, verb: m[1], cmd })
  }
  return out
}

/** 'false', 'true', or null when the command leaves Latest to GitHub. */
export function latestFlag(cmd) {
  if (/(^|\s)--latest=false(?=\s|$)/.test(cmd)) return 'false'
  if (/(^|\s)--latest(=true)?(?=\s|$)/.test(cmd)) return 'true'
  return null
}

/** The first line (1-based) at or after which `needle` appears, or 0. */
function lineOf(text, needle) {
  const i = text.split('\n').findIndex((l) => !/^\s*#/.test(l) && l.includes(needle))
  return i + 1
}

const STEP_START = /^\s*-\s+(name|uses):/
const MINT_TEST = /\[\s*"?\$\{?MINT\}?"?\s*==?\s*"?true"?\s*\]/
const MINT_BOUND = /^\s*MINT:\s*\$\{\{\s*needs\.resolve\.outputs\.mint\s*\}\}\s*$/
const STEP_IF_MINT = /^\s*if:\s*(\$\{\{\s*)?needs\.resolve\.outputs\.mint\s*==\s*'true'\s*(\}\})?\s*$/

/**
 * Why the command at line `n` (1-based) is NOT confined to the mint path, or
 * null when it is. Confined means: the step binds MINT to resolve's output,
 * and the command sits in the `then` branch of a shell `if` whose test is
 * "$MINT" = true (an `&&` with more tests is fine, an `||` is not), or the
 * step itself has `if: needs.resolve.outputs.mint == 'true'`.
 */
export function mintGuard(text, n) {
  const lines = text.split('\n')
  let start = n - 1
  while (start > 0 && !STEP_START.test(lines[start])) start--
  const step = lines.slice(start, n)
  if (step.some((l) => STEP_IF_MINT.test(l))) return null
  if (!step.some((l) => MINT_BOUND.test(l))) {
    return 'its step does not bind MINT to needs.resolve.outputs.mint, so no `$MINT` test can confine it'
  }
  let depth = 0
  let branch = 'then'
  for (let i = n - 2; i > start; i--) {
    const t = lines[i].trim()
    if (t.startsWith('#') || t === '') continue
    if (/^run:/.test(t)) break
    if (/^fi\b/.test(t)) {
      depth++
      continue
    }
    if (/^if\s/.test(t)) {
      if (/\bfi\s*;?\s*$/.test(t)) continue // a one-line if … fi above it
      if (depth > 0) {
        depth--
        continue
      }
      if (branch !== 'then') return `it runs in the else branch of \`${t}\``
      if (!MINT_TEST.test(t) || t.includes('||')) return `its \`if\` does not test "$MINT" = true on its own: \`${t}\``
      return null
    }
    if (depth === 0 && /^(else|elif)\b/.test(t)) branch = 'else'
  }
  return 'it is not inside an `if [ "$MINT" = true ]`'
}

/**
 * @param {Map<string, string>} workflows  file name (e.g. 'release.yml') → text
 * @param {string | null} tauriConf  desktop/src-tauri/tauri.conf.json
 * @returns {{ path: string, line: number, text: string }[]}
 */
export function checkReleaseLatest(workflows, tauriConf) {
  const found = []
  const at = (file, line, text) => found.push({ path: `.github/workflows/${file}`, line, text })

  // The premise. If the updater stops reading the Latest release, this rule
  // is about nothing, and whoever moved it has to decide what replaces it.
  let endpoints = []
  try {
    endpoints = JSON.parse(tauriConf ?? 'null')?.plugins?.updater?.endpoints ?? []
  } catch {
    endpoints = []
  }
  if (!endpoints.some((e) => typeof e === 'string' && e.endsWith(FEED_SUFFIX))) {
    found.push({
      path: 'desktop/src-tauri/tauri.conf.json',
      line: 0,
      text: `the updater no longer reads ...${FEED_SUFFIX}, which is what makes the Latest flag the feed`,
    })
  }

  for (const [file, text] of workflows) {
    const cmds = releaseCommands(text)
    for (const c of cmds) {
      const flag = latestFlag(c.cmd)
      if (c.verb === 'create' && flag === null && !/(^|\s)--prerelease(?=\s|$)/.test(c.cmd)) {
        at(file, c.line, `\`gh release create\` leaves Latest to GitHub, which marks a new release Latest by default: ${c.cmd}`)
      }
      if (flag === 'true' && !(file === DESKTOP && c.verb === 'edit')) {
        at(file, c.line, `only desktop-package.yml's feed flip may mark a release Latest: ${c.cmd}`)
      }
      if ((file === SERVER || file === DESKTOP) && c.verb === 'create' && flag !== 'false') {
        at(file, c.line, `must be created --latest=false: ${c.cmd}`)
      }
      if (file === SERVER && c.verb === 'edit' && flag !== 'false') {
        at(file, c.line, `a re-run's edit must say --latest=false too: ${c.cmd}`)
      }
    }
    if (file === SERVER && !cmds.some((c) => c.verb === 'create')) {
      at(file, 0, 'opens no release any more: the rule has lost its subject — move it with the release step')
    }
    if (file === DESKTOP) {
      if (!cmds.some((c) => c.verb === 'create')) {
        at(file, 0, 'mints no release any more: the rule has lost its subject — move it with the mint')
      }
      const flips = cmds.filter((c) => c.verb === 'edit' && latestFlag(c.cmd) === 'true')
      const feed = lineOf(text, 'latest.json --clobber')
      if (flips.length === 0) {
        at(file, 0, 'the minted desktop-v release is never marked Latest: no `gh release edit ... --latest`')
      } else if (feed === 0) {
        at(file, 0, 'no `latest.json --clobber` upload to order the Latest flip after')
      } else {
        for (const f of flips) {
          if (f.line < feed) at(file, f.line, `marks the release Latest before its latest.json is uploaded (line ${feed}): the feed would 404 in between`)
          const why = mintGuard(text, f.line)
          if (why) {
            at(file, f.line, `the Latest flip can run off the mint path — ${why}. \`attach\` also runs for a re-attach dispatch, which must never move the feed`)
          }
        }
      }
    }
  }
  if (!workflows.has(SERVER)) found.push({ path: `.github/workflows/${SERVER}`, line: 0, text: 'missing' })
  if (!workflows.has(DESKTOP)) found.push({ path: `.github/workflows/${DESKTOP}`, line: 0, text: 'missing' })
  return found
}

/** The check against fixtures: [] when it judges every one right. */
export function selfTest() {
  const wrong = []
  const conf = JSON.stringify({ plugins: { updater: { endpoints: [`https://github.com/o/r${FEED_SUFFIX}`] } } })
  const server = [
    'run: |',
    '  # gh release create "$TAG" in a comment is not a command',
    '  if gh release view "$TAG" >/dev/null 2>&1; then',
    '    gh release edit "$TAG" --title "$TAG" --notes "$notes" --latest=false "${pre[@]}"',
    '  else',
    '    gh release create "$TAG" --verify-tag --title "$TAG" \\',
    '      --notes "$notes" --latest=false "${pre[@]}"',
    '  fi',
  ].join('\n')
  const desktop = [
    '- name: Open the desktop release',
    '  run: |',
    '    gh release create "$TAG" --repo "$R" \\',
    '      --target "$SHA" --title "$TAG" --latest=false \\',
    '      --notes "x"',
    '- name: Upload',
    '  env:',
    '    TAG: ${{ needs.resolve.outputs.release_tag }}',
    '    MINT: ${{ needs.resolve.outputs.mint }}',
    '  run: |',
    '    gh release upload "$TAG" --repo "$R" assets/* --clobber',
    '    if [ -f ./latest.json ]; then',
    '      gh release upload "$TAG" --repo "$R" latest.json --clobber',
    '    fi',
    '    # if [ "$MINT" = true ]; then  (a comment is not a guard)',
    '    if [ "$MINT" = true ]; then',
    '      gh release edit "$TAG" --repo "$R" --latest',
    '      echo flipped',
    '    fi',
  ].join('\n')
  const FLIP = '      gh release edit "$TAG" --repo "$R" --latest\n'
  const GUARDED = `    if [ "$MINT" = true ]; then\n${FLIP}      echo flipped\n    fi`
  const mobile = 'run: |\n  gh release create "$TAG" --target "$SHA" --title "$TAG" --prerelease --notes-file notes.md\n'
  const run = (over = {}, c = conf) => {
    const w = new Map([
      [SERVER, server],
      [DESKTOP, desktop],
      ['mobile-package.yml', mobile],
    ])
    for (const [k, v] of Object.entries(over)) {
      if (v === null) w.delete(k)
      else w.set(k, v)
    }
    return checkReleaseLatest(w, c)
  }
  const expect = (label, got, n) => {
    if (got.length !== n) wrong.push(`${label}: expected ${n} finding(s), got ${got.length}${got.length ? ` (${got.map((f) => f.text).join(' | ')})` : ''}`)
  }
  expect('the shapes this repo ships', run(), 0)
  expect('server edit without --latest=false', run({ [SERVER]: server.replace('--notes "$notes" --latest=false "${pre[@]}"\n  else', '--notes "$notes" "${pre[@]}"\n  else') }), 1)
  expect('server create without the flag (on its continuation line)', run({ [SERVER]: server.replace('--notes "$notes" --latest=false "${pre[@]}"\n  fi', '--notes "$notes" "${pre[@]}"\n  fi') }), 2)
  expect('server create marked --latest', run({ [SERVER]: server.replace('--latest=false "${pre[@]}"\n  fi', '--latest "${pre[@]}"\n  fi') }), 2)
  expect('server with no release step', run({ [SERVER]: 'run: echo nothing' }), 1)
  expect('desktop mint left to GitHub', run({ [DESKTOP]: desktop.replace(' --latest=false \\', ' \\') }), 2)
  expect('desktop never flips Latest', run({ [DESKTOP]: desktop.replace(FLIP, '') }), 1)
  const early = desktop.replace(GUARDED, '').replace(
    '    gh release upload "$TAG" --repo "$R" assets/* --clobber',
    `${GUARDED}\n    gh release upload "$TAG" --repo "$R" assets/* --clobber`,
  )
  expect('desktop flips Latest before latest.json lands', run({ [DESKTOP]: early }), 1)
  // The flip off the mint path: attach runs for release.yml's v* tags too.
  expect('desktop flip with its MINT guard removed', run({ [DESKTOP]: desktop.replace(GUARDED, `${FLIP}      echo flipped`) }), 1)
  expect(
    "desktop flip in the guard's else branch",
    run({ [DESKTOP]: desktop.replace(GUARDED, `    if [ "$MINT" = true ]; then\n      echo minted\n    else\n${FLIP}    fi`) }),
    1,
  )
  expect('desktop flip guarded by the wrong test', run({ [DESKTOP]: desktop.replace(GUARDED, GUARDED.replace('[ "$MINT" = true ]', '[ -f ./latest.json ]')) }), 1)
  expect('desktop flip guarded by an || test', run({ [DESKTOP]: desktop.replace(GUARDED, GUARDED.replace('[ "$MINT" = true ]', '[ "$MINT" = true ] || [ -n "$TAG" ]')) }), 1)
  expect('desktop flip with MINT not bound to resolve', run({ [DESKTOP]: desktop.replace('MINT: ${{ needs.resolve.outputs.mint }}', "MINT: 'true'") }), 1)
  expect(
    'desktop flip after an unrelated if … fi, guard removed',
    run({ [DESKTOP]: desktop.replace(GUARDED, `    if [ -n "$TAG" ]; then\n      echo tag\n    fi\n${FLIP}`) }),
    1,
  )
  expect('desktop flip with an && guard', run({ [DESKTOP]: desktop.replace(GUARDED, GUARDED.replace('[ "$MINT" = true ]', '[ "$MINT" = true ] && [ -f ./latest.json ]')) }), 0)
  expect(
    'desktop flip under a step-level mint if:',
    run({ [DESKTOP]: desktop.replace(GUARDED, FLIP).replace("- name: Upload\n", "- name: Upload\n  if: needs.resolve.outputs.mint == 'true'\n") }),
    0,
  )
  expect('another workflow marks a release Latest', run({ 'mobile-package.yml': mobile.replace('--prerelease', '--latest') }), 1)
  expect('another workflow leaves Latest to GitHub', run({ 'mobile-package.yml': mobile.replace(' --prerelease', '') }), 1)
  expect('the updater reads another URL', run({}, JSON.stringify({ plugins: { updater: { endpoints: ['https://example.test/feed.json'] } } })), 1)
  expect('a workflow file gone', run({ [DESKTOP]: null }), 1)
  if (latestFlag('gh release edit "$T" --latest=false') !== 'false') wrong.push('latestFlag misreads --latest=false')
  if (latestFlag('gh release edit "$T" --latest') !== 'true') wrong.push('latestFlag misreads --latest')
  if (latestFlag('gh release edit "$T" --latest=true') !== 'true') wrong.push('latestFlag misreads --latest=true')
  if (latestFlag('gh release create "$T" --notes "the latest build"') !== null) wrong.push('latestFlag reads prose as a flag')
  return wrong
}

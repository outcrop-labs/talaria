// Is a gear drawn anywhere but from the one settings cog?
//
// THE RULE. Every door into a settings surface wears ONE gear, from
// ui/src/components/ui/settings-cog.ts: `SettingsButton` for an icon-only door,
// `cogMenuIcon` for a menu entry, `SettingsGlyph` beside a visible label. Before
// that file, three glyphs meant "settings" (lucide Settings, Settings2 and
// SlidersHorizontal) and the board, the list's columns and the comms header
// each drew their own. A per-view copy is one import away, so this fails it.
//
// WHAT IT FLAGS, per file, by the lucide name the import resolves to (an
// `as` alias renames nothing):
//   · a named import OR re-export from '@lucide/svelte' whose name contains
//     Settings, Cog or Sliders — Settings, Settings2, Cog, SlidersHorizontal,
//     Sliders (the alias of sliders-vertical), ColumnsSettings and Columns3Cog
//     (a column gear), UserCog and the rest of the *Cog family, and the
//     SettingsIcon / LucideSettings spellings. A re-export
//     (`export { Settings as Gear } from '@lucide/svelte'`) is the cheapest
//     second home for the gear: every view could then import `Gear` from it;
//   · `export * from '@lucide/svelte'` (or `export * as L`), which re-exports
//     the gear with everything else;
//   · the same names read off a namespace import (`import * as L`, then
//     `L.Settings`) or off lucide's `icons` map (`icons.Cog`);
//   · a deep import or re-export, '@lucide/svelte/icons/<slug>' or
//     '@lucide/svelte/dist/icons/<slug>', whose slug contains settings, cog
//     or sliders;
//   · lucide's gear path data pasted into a file (`M9.671 4.136`, and
//     `M12.22 2h-.44` from older releases) — an inline SVG copy.
// The sliders are flagged ON PURPOSE, though they read as adjust or filter:
// two of the six settings doors wore them, so a new one is where a settings
// door comes back. A sliders control that is not a settings door says so with
// its allowlist entry below.
//
// KNOWN LIMITS. `icons[name]` with a computed name, a dynamic
// `import('@lucide/svelte')` (its members are read off a value, not an import
// statement), and lucide's `Icon` fed a hand-written iconNode that is not
// lucide's gear path, are not seen. None exists in the tree.
//
// No imports: plain string work, runnable with no install like the rest of
// scripts/. check-invariants.mjs wires it to the tree, after selfTest().

/** The ONLY files that may import one of these names, and which names — each
 *  entry says why that use is not a settings door drawing its own gear. */
export const SETTINGS_GLYPH_ALLOW = {
  // The one home of the gear.
  'ui/src/components/ui/settings-cog.ts': ['Settings'],
  // The Agent Studio nav glyph: a destination in the nav, not a door into
  // the settings of the thing in view.
  'ui/src/lib/nav.ts': ['Settings2'],
  // The dock's Manage tile: app chrome, a destination (the settings area),
  // kept on its own spoked cog so it is not read as one view's settings.
  'ui/src/components/app/TabBar.svelte': ['Cog'],
  // The account menu: app chrome. Its "Settings" row and its "Manage" row are
  // destinations at the chrome row scale (15px), not doors in a view.
  'ui/src/components/app/UserMenu.svelte': ['Settings', 'Cog'],
  // "Ticket properties": opens the ticket's fields drawer — the record's own
  // properties, not settings.
  'ui/src/components/board/TaskDetail.svelte': ['SlidersHorizontal'],
}

const GEAR_NAME = /Settings|Cog|Sliders/
const GEAR_SLUG = /settings|cog|sliders/
const GEAR_PATHS = ['M9.671 4.136', 'M12.22 2h-.44']
const LUCIDE = `['"]@lucide\\/svelte['"]`

const lineOf = (src, index) => src.slice(0, index).split('\n').length

/** Every gear the source draws: `{ line, name, text }`, where `name` is the
 *  lucide export (or `path data` for a pasted SVG). Comments are the caller's
 *  to strip; this reads what it is given. */
export function scanSettingsGlyphs(src) {
  const hits = []
  const seen = new Set()
  const hit = (index, name, text) => {
    const key = `${name}@${lineOf(src, index)}`
    if (seen.has(key)) return
    seen.add(key)
    hits.push({ line: lineOf(src, index), name, text })
  }
  // Named imports and re-exports, multi-line included:
  // `import { A, B as C } from '@lucide/svelte'`, `export { A as B } from …`.
  const maps = []
  for (const m of src.matchAll(new RegExp(`\\b(import|export)\\s+(?:type\\s+)?\\{([^}]*)\\}\\s*from\\s*${LUCIDE}`, 'g'))) {
    const verb = m[1]
    for (const piece of m[2].split(',')) {
      const t = piece.trim().replace(/^type\s+/, '')
      if (!t) continue
      const [name, local] = t.split(/\s+as\s+/).map((x) => x.trim())
      if (name === 'icons' && verb === 'import') maps.push(local ?? name)
      else if (GEAR_NAME.test(name)) hit(m.index, name, `${verb} { ${t} } from '@lucide/svelte'`)
    }
  }
  // A star re-export hands the gear on with every other icon.
  for (const m of src.matchAll(new RegExp(`\\bexport\\s+\\*\\s+(?:as\\s+[A-Za-z_$][\\w$]*\\s+)?from\\s*${LUCIDE}`, 'g'))) {
    hit(m.index, 'export *', `${m[0].replace(/\s+/g, ' ')} (re-exports the gear)`)
  }
  // Namespace imports, and lucide's own `icons` map, read by member.
  for (const m of src.matchAll(new RegExp(`import\\s+\\*\\s+as\\s+([A-Za-z_$][\\w$]*)\\s+from\\s*${LUCIDE}`, 'g'))) maps.push(m[1])
  for (const ns of maps) {
    for (const m of src.matchAll(new RegExp(`\\b${ns.replace(/\$/g, '\\$')}\\s*(?:\\.\\s*([A-Za-z_$][\\w$]*)|\\[\\s*['"]([^'"]+)['"]\\s*\\])`, 'g'))) {
      const name = m[1] ?? m[2]
      if (GEAR_NAME.test(name)) hit(m.index, name, `${ns}.${name}`)
    }
  }
  // Deep imports (and re-exports) by icon slug, the package's dist/ path too.
  for (const m of src.matchAll(/['"]@lucide\/svelte\/((?:dist\/)?icons)\/([\w-]+)(?:\.js|\.svelte)?['"]/g)) {
    if (GEAR_SLUG.test(m[2])) hit(m.index, m[2], `@lucide/svelte/${m[1]}/${m[2]}`)
  }
  // A pasted gear.
  for (const p of GEAR_PATHS) {
    let i = src.indexOf(p)
    while (i !== -1) {
      hit(i, 'path data', `lucide gear path data (${p})`)
      i = src.indexOf(p, i + p.length)
    }
  }
  return hits
}

/** The whole judgement over a tree: `sources` maps a repo-relative path to its
 *  (comment-stripped) text. `violations` are gears outside the allowlist;
 *  `stale` are allowlist entries whose file or name is gone — an exemption
 *  held open for nothing is the next copy's way in. */
export function judgeSettingsGlyphs(sources, allow = SETTINGS_GLYPH_ALLOW) {
  const violations = []
  const used = new Map()
  for (const [path, src] of sources) {
    const allowed = allow[path] ?? []
    for (const h of scanSettingsGlyphs(src)) {
      if (allowed.includes(h.name)) {
        if (!used.has(path)) used.set(path, new Set())
        used.get(path).add(h.name)
      } else violations.push({ path, line: h.line, text: h.text })
    }
  }
  const stale = []
  for (const [path, names] of Object.entries(allow)) {
    if (!sources.has(path)) {
      stale.push({ path, line: 0, text: 'on the allowlist but no longer exists' })
      continue
    }
    for (const name of names) {
      if (!used.get(path)?.has(name)) stale.push({ path, line: 0, text: `allowlisted for ${name}, which it no longer imports` })
    }
  }
  return { violations, stale }
}

/** The fixtures: what the rule exists to catch, and what it must let pass.
 *  Answers the list of what came out wrong (empty is a pass). */
export function selfTest() {
  const wrong = []
  const names = (src) => scanSettingsGlyphs(src).map((h) => h.name)
  const expect = (label, got, want) => {
    if (JSON.stringify(got) !== JSON.stringify(want)) wrong.push(`${label}: expected ${JSON.stringify(want)}, got ${JSON.stringify(got)}`)
  }
  expect('a multi-line import', names("import {\n  Plus,\n  Settings\n} from '@lucide/svelte'"), ['Settings'])
  expect('an aliased import', names("import { Settings as Gear } from '@lucide/svelte'"), ['Settings'])
  expect('the sliders and the old board glyph', names("import { Settings2, SlidersHorizontal, Sliders } from '@lucide/svelte'"), ['Settings2', 'SlidersHorizontal', 'Sliders'])
  expect('the gear-badged family', names("import { ColumnsSettings, Columns3Cog, UserCog, SettingsIcon, LucideCog } from \"@lucide/svelte\""), ['ColumnsSettings', 'Columns3Cog', 'UserCog', 'SettingsIcon', 'LucideCog'])
  expect('a deep import', names("import Cog from '@lucide/svelte/icons/cog'"), ['cog'])
  expect('a deep import from dist', names("import Gear from '@lucide/svelte/dist/icons/settings.svelte'"), ['settings'])
  expect('a re-export', names("export { Settings } from '@lucide/svelte'"), ['Settings'])
  expect('an aliased, multi-line re-export', names("export {\n  Check,\n  Settings as Gear,\n} from '@lucide/svelte'"), ['Settings'])
  expect('a deep re-export', names("export { default as Gear } from '@lucide/svelte/icons/settings'"), ['settings'])
  expect('a star re-export', names("export * from '@lucide/svelte'\nexport * as L from \"@lucide/svelte\""), ['export *', 'export *'])
  expect('a re-export that is not a gear', names("export { Check, Plus } from '@lucide/svelte'"), [])
  expect('a namespace import', names("import * as L from '@lucide/svelte'\n<L.Settings size={14} />\n<L.Check />"), ['Settings'])
  expect("lucide's icons map", names("import { icons } from '@lucide/svelte'\nconst G = icons.Cog\nconst H = icons['Settings2']"), ['Cog', 'Settings2'])
  expect('pasted path data', names('<svg><path d="M9.671 4.136a2.34 2.34 0 0 1 4.659 0" /></svg>'), ['path data'])
  expect('icons that are not gears', names("import { Check, Plus, ListFilter, Ellipsis } from '@lucide/svelte'\nimport Check2 from '@lucide/svelte/icons/check'"), [])
  expect('a gear from the shared module', names("import { SettingsGlyph, cogMenuIcon } from '@/components/ui/settings-cog'"), [])

  const allow = { 'ui/src/lib/nav.ts': ['Settings2'] }
  const tree = (extra) => new Map([['ui/src/lib/nav.ts', "import { Settings2 } from '@lucide/svelte'"], ...extra])
  const clean = judgeSettingsGlyphs(tree([]), allow)
  expect('an allowlisted name in its file', [clean.violations.length, clean.stale.length], [0, 0])
  const copy = judgeSettingsGlyphs(tree([['ui/src/x.svelte', "import {\n  Plus,\n  Settings\n} from '@lucide/svelte'"]]), allow)
  expect('a gear outside the allowlist', copy.violations.map((v) => `${v.path}:${v.line}`), ['ui/src/x.svelte:1'])
  const other = judgeSettingsGlyphs(new Map([['ui/src/lib/nav.ts', "import { Settings2, Cog } from '@lucide/svelte'"]]), allow)
  expect('an allowlisted file importing a name it is not allowed', other.violations.map((v) => v.text), ["import { Cog } from '@lucide/svelte'"])
  const gone = judgeSettingsGlyphs(new Map([['ui/src/lib/nav.ts', "import { Check } from '@lucide/svelte'"]]), allow)
  expect('an allowlist entry whose name is no longer imported', gone.stale.map((s) => s.path), ['ui/src/lib/nav.ts'])
  const missing = judgeSettingsGlyphs(new Map(), allow)
  expect('an allowlist entry whose file is gone', missing.stale.map((s) => s.text), ['on the allowlist but no longer exists'])
  return wrong
}

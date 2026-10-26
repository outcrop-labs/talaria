// An app's manifest, talaria.json — the TS host's reading of the ONE set of
// rules. Rust's talaria-app-manifest is the validator install, update and
// discovery run (api/crates/talaria-app-manifest); this file holds the TS
// host to the same judgement, so an app the api lists as broken is one the
// host never builds or mounts. api/tests/it/fixtures/app-manifest.json pins
// the two (manifest.test.ts here, the crate's tests there), the SDK version
// included; docs/sdk/talaria.schema.json publishes the shape.

/** The version of `@talaria/sdk` this host provides. A manifest's `sdk`
 *  range must admit it. Minor bumps add to the SDK, major bumps break it.
 *  Rust twin: `SDK_VERSION` in talaria-app-manifest; both equal the shared
 *  fixture's `sdkVersion`. */
export const SDK_VERSION = '1.0.0'

/** Apps' slug rule (Rust `slug_ok`). */
export const SLUG_RE = /^[a-z0-9][a-z0-9-]{0,63}$/

/** Slugs no new app may take (Rust `RESERVED_APP_SLUGS`; the shared
 *  app-grants fixture's `routes.reservedSlugs`). */
export const RESERVED_APP_SLUGS: readonly string[] = ['layout', 'hooks', 'meta']

export const SURFACES = ['work', 'manage', 'settings'] as const
export type SurfaceKey = (typeof SURFACES)[number]

/** Every key the manifest gives a meaning to; any other is ignored. */
export const KNOWN_KEYS = ['$schema', 'name', 'slug', 'icon', 'version', 'description', 'surfaces', 'sdk', 'harnesses', 'triggers'] as const

export const LIMITS = { name: 120, surfaceTitle: 120, icon: 16, version: 64, description: 2000 } as const

export interface AppManifest {
  name: string
  slug?: string
  icon: string
  description: string
  version: string
  surfaces: Partial<Record<SurfaceKey, string>>
  sdk?: string
}

export type ManifestResult = { ok: true; manifest: AppManifest } | { ok: false; error: string }

/** Length in Unicode code points, as Rust's `chars().count()`. */
const chars = (s: string): number => [...s].length

function slugInstallable(s: string): string | null {
  if (!SLUG_RE.test(s)) return `"${s}" is not a usable app slug (lowercase letters, digits, dashes)`
  if (RESERVED_APP_SLUGS.includes(s)) {
    return `"${s}" is reserved: the client router uses that word in its route table — install the app under another slug`
  }
  return null
}

const isObject = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v)

/** The rules, on a parsed value (Rust `validate_manifest`). */
export function validateManifest(value: unknown, hostSdk: string = SDK_VERSION): ManifestResult {
  const fail = (error: string): ManifestResult => ({ ok: false, error })
  if (!isObject(value)) return fail('talaria.json must be a JSON object')
  const name = value.name
  if (typeof name !== 'string' || !name.trim()) return fail('talaria.json needs a "name": a non-empty string')
  if (chars(name) > LIMITS.name) return fail(`"name" is longer than ${LIMITS.name} characters`)

  let slug: string | undefined
  if (value.slug !== undefined && value.slug !== null) {
    if (typeof value.slug !== 'string') return fail('"slug" must be a string')
    const why = slugInstallable(value.slug)
    if (why) return fail(`"slug": ${why}`)
    slug = value.slug
  }

  const optional = (key: string, max: number): { ok: true; value?: string } | { ok: false; error: string } => {
    const v = value[key]
    if (v === undefined || v === null) return { ok: true }
    if (typeof v !== 'string') return { ok: false, error: `"${key}" must be a string` }
    if (chars(v) > max) return { ok: false, error: `"${key}" is longer than ${max} characters` }
    return { ok: true, value: v }
  }
  const icon = optional('icon', LIMITS.icon)
  if (!icon.ok) return fail(icon.error)
  const description = optional('description', LIMITS.description)
  if (!description.ok) return fail(description.error)
  const version = optional('version', LIMITS.version)
  if (!version.ok) return fail(version.error)

  const surfaces: Partial<Record<SurfaceKey, string>> = {}
  if (value.surfaces !== undefined && value.surfaces !== null) {
    if (!isObject(value.surfaces)) return fail('"surfaces" must be an object')
    for (const [key, title] of Object.entries(value.surfaces)) {
      if (!(SURFACES as readonly string[]).includes(key)) {
        return fail(`"surfaces" names an unknown surface "${key}" (expected work, manage or settings)`)
      }
      if (typeof title !== 'string' || !title.trim()) {
        return fail(`"surfaces.${key}" must be a non-empty string (the surface's title)`)
      }
      if (chars(title) > LIMITS.surfaceTitle) return fail(`"surfaces.${key}" is longer than ${LIMITS.surfaceTitle} characters`)
      surfaces[key as SurfaceKey] = title
    }
  }

  let sdk: string | undefined
  if (value.sdk !== undefined && value.sdk !== null) {
    if (typeof value.sdk !== 'string') return fail('"sdk" must be a string (a version range such as "^1.0")')
    const judged = satisfiesRange(value.sdk, hostSdk)
    if (!judged.ok) return fail(`"sdk" is not a version range: ${judged.error}`)
    if (!judged.satisfied) return fail(`this app needs @talaria/sdk ${value.sdk}; this host provides ${hostSdk}`)
    sdk = value.sdk
  }

  const h = value.harnesses
  if (h !== undefined && h !== null && typeof h !== 'object') {
    return fail('"harnesses" must be an array or an object (reserved for the harness registry)')
  }
  const t = value.triggers
  if (t !== undefined && t !== null) {
    if (!Array.isArray(t)) return fail('"triggers" must be an array')
    for (const [i, trigger] of t.entries()) {
      if (!isObject(trigger)) return fail(`"triggers[${i}]" must be an object`)
      if (typeof trigger.id !== 'string' || !SLUG_RE.test(trigger.id)) {
        return fail(`"triggers[${i}].id" must be a lowercase id (letters, digits, dashes)`)
      }
      // Exactly one of the two keys, whatever its value — as the schema's
      // oneOf reads it — and that one a non-blank string.
      const hasCron = Object.hasOwn(trigger, 'cron')
      if (hasCron === Object.hasOwn(trigger, 'event')) return fail(`"triggers[${i}]" needs exactly one of "cron" or "event"`)
      const key = hasCron ? 'cron' : 'event'
      const v = trigger[key]
      if (typeof v !== 'string' || !v.trim()) return fail(`"triggers[${i}].${key}" must be a string that is not only whitespace`)
    }
  }

  return {
    ok: true,
    manifest: {
      name,
      ...(slug ? { slug } : {}),
      icon: icon.value || '⬡',
      description: description.value ?? '',
      version: version.value || '0.0.0',
      surfaces,
      ...(sdk !== undefined ? { sdk } : {}),
    },
  }
}

// A manifest that is not UTF-8 is not an app: fatal, and the BOM kept so a
// byte-order mark fails JSON.parse the way it fails serde_json.
const MANIFEST_DECODER = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true })

/** Read and validate a manifest file's bytes (Rust `parse_manifest`). */
export function parseManifest(bytes: Uint8Array, hostSdk: string = SDK_VERSION): ManifestResult {
  let text: string
  try {
    text = MANIFEST_DECODER.decode(bytes)
  } catch {
    return { ok: false, error: 'talaria.json is not UTF-8' }
  }
  let value: unknown
  try {
    value = JSON.parse(text)
  } catch (e) {
    return { ok: false, error: `talaria.json is not valid JSON: ${e instanceof Error ? e.message : String(e)}` }
  }
  return validateManifest(value, hostSdk)
}

// ── Version ranges (Rust `satisfies`) ───────────────────────────────────────

type Version = [number, number, number]
type Bound = { op: '>=' | '>' | '<' | '<='; v: Version }

const cmp = (a: Version, b: Version): number => a[0] - b[0] || a[1] - b[1] || a[2] - b[2]

function admits(b: Bound, v: Version): boolean {
  const c = cmp(v, b.v)
  return b.op === '>=' ? c >= 0 : b.op === '>' ? c > 0 : b.op === '<' ? c < 0 : c <= 0
}

export function parseVersion(s: string): Version | null {
  const parts = s.trim().split('.')
  if (parts.length !== 3 || !parts.every((p) => /^\d+$/.test(p))) return null
  return parts.map(Number) as Version
}

type Partial3 = [number | null, number | null, number | null]

function parsePartial(s: string): Partial3 | string {
  if (s.includes('-') || s.includes('+')) return `"${s}": pre-release and build tags are not supported`
  const parts = s.split('.')
  if (parts.length === 0 || parts.length > 3) return `"${s}" is not a version`
  const out: Partial3 = [null, null, null]
  let open = false
  for (const [i, p] of parts.entries()) {
    if (p === 'x' || p === 'X' || p === '*') {
      open = true
      continue
    }
    if (open) return `"${s}": a number cannot follow a wildcard`
    if (!/^\d+$/.test(p)) return `"${s}" is not a version`
    out[i] = Number(p)
  }
  return out
}

function comparator(op: string, partial: string): Bound[] | string {
  const parsed = parsePartial(partial)
  if (typeof parsed === 'string') return parsed
  const [maj, min, pat] = parsed
  if (maj === null) return ['', '=', '>=', '^', '~'].includes(op) ? [] : `"${op}${partial}" admits no version`
  const lower: Version = [maj, min ?? 0, pat ?? 0]
  const nextMajor: Version = [maj + 1, 0, 0]
  const nextMinor = (m: number): Version => [maj, m + 1, 0]
  const ge = (v: Version): Bound => ({ op: '>=', v })
  const lt = (v: Version): Bound => ({ op: '<', v })
  switch (op) {
    case '':
    case '=':
      if (min === null) return [ge(lower), lt(nextMajor)]
      if (pat === null) return [ge(lower), lt(nextMinor(min))]
      return [ge(lower), { op: '<=', v: lower }]
    case '^':
      if (min === null) return [ge(lower), lt(nextMajor)]
      if (pat === null) return maj > 0 ? [ge(lower), lt(nextMajor)] : [ge(lower), lt(nextMinor(min))]
      if (maj > 0) return [ge(lower), lt(nextMajor)]
      if (min > 0) return [ge(lower), lt(nextMinor(min))]
      return [ge(lower), lt([0, 0, pat + 1])]
    case '~':
      return min === null ? [ge(lower), lt(nextMajor)] : [ge(lower), lt(nextMinor(min))]
    case '>=':
      return [ge(lower)]
    case '>':
      if (min === null) return [ge(nextMajor)]
      if (pat === null) return [ge(nextMinor(min))]
      return [{ op: '>', v: lower }]
    case '<':
      return [lt(lower)]
    case '<=':
      if (min === null) return [lt(nextMajor)]
      if (pat === null) return [lt(nextMinor(min))]
      return [{ op: '<=', v: lower }]
  }
  return `"${op}" is not a range operator`
}

const OPS = ['>=', '<=', '>', '<', '=', '^', '~']

function splitOp(token: string): [string, string] {
  for (const op of OPS) if (token.startsWith(op)) return [op, token.slice(op.length)]
  return ['', token]
}

function comparatorSet(set: string): Bound[] | string {
  const tokens = set.split(/\s+/).filter(Boolean)
  const bounds: Bound[] = []
  for (let i = 0; i < tokens.length; i++) {
    if (tokens[i] === '-') return 'hyphen ranges (a - b) are not supported; write >=a <=b'
    let [op, partial] = splitOp(tokens[i]!)
    if (!partial && op) {
      i++
      const next = tokens[i]
      if (next === undefined) return `"${op}" needs a version after it`
      partial = next
    }
    if (!partial) return `"${tokens[i]}" is not a version`
    if (splitOp(partial)[0]) return `"${tokens[i]}" has two operators`
    const got = comparator(op, partial)
    if (typeof got === 'string') return got
    bounds.push(...got)
  }
  return bounds
}

/** Whether `version` is in `range`, or why the range cannot be read. */
export function satisfiesRange(range: string, version: string): { ok: true; satisfied: boolean } | { ok: false; error: string } {
  const v = parseVersion(version)
  if (!v) return { ok: false, error: `"${version}" is not a version (major.minor.patch)` }
  const r = range.trim()
  if (r.length > 256) return { ok: false, error: 'the range is longer than 256 characters' }
  let any = false
  for (const set of r.split('||')) {
    const bounds = comparatorSet(set)
    if (typeof bounds === 'string') return { ok: false, error: bounds }
    if (bounds.every((b) => admits(b, v))) any = true
  }
  return { ok: true, satisfied: any }
}

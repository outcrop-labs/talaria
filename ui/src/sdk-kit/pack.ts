// The kit builder's pure parts: which source files the SDK's declarations
// need, how a declaration's `@/` imports become relative ones, and the tar
// the kit ships as. ui/scripts/build-sdk-kit.ts drives them; they live here
// so they are tested with the rest of the ui.
import { createHash } from 'node:crypto'
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs'
import { dirname, join, relative, resolve } from 'node:path'
import { gzipSync } from 'node:zlib'

/** The SDK's three entries, relative to ui/src. */
export const SDK_ENTRIES = ['sdk/index.ts', 'sdk/server.ts', 'sdk/testing.ts'] as const

const stripComments = (code: string): string => code.replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|[^:'"`\\])\/\/.*$/gm, '$1')

/** Every specifier a module imports or re-exports by a static statement —
 *  type-only ones included, because a declaration needs the types — read out
 *  of a .ts file or a .svelte file's scripts. Dynamic `import()` is lazy code,
 *  not part of a declaration, and is not followed. */
export function staticSpecifiers(file: string, code: string): string[] {
  const src = stripComments(file.endsWith('.svelte') ? [...code.matchAll(/<script[^>]*>([\s\S]*?)<\/script>/g)].map((m) => m[1]).join('\n') : code)
  const out: string[] = []
  for (const m of src.matchAll(/\b(?:import|export)\s+(?:type\s+)?[^;'"()]*?\s*from\s*['"]([^'"]+)['"]/g)) if (m[1]) out.push(m[1])
  for (const m of src.matchAll(/\bimport\s*['"]([^'"]+)['"]/g)) if (m[1]) out.push(m[1])
  return out
}

/** `spec` imported from `from`, as a file under `src` — null for a package. */
export function resolveLocal(src: string, spec: string, from: string): string | null {
  const base = spec.startsWith('@/') ? join(src, spec.slice(2)) : spec.startsWith('.') ? resolve(dirname(from), spec) : null
  if (!base) return null
  for (const candidate of [base, `${base}.ts`, `${base}.js`, join(base, 'index.ts')]) {
    if (existsSync(candidate) && statSync(candidate).isFile()) return candidate
  }
  return null
}

/** The files under `src` the SDK entries reach by static imports, relative
 *  to `src`, sorted. */
export function sdkSourceClosure(src: string, entries: readonly string[] = SDK_ENTRIES): string[] {
  const start = entries.map((e) => join(src, e))
  const seen = new Set(start)
  const queue = [...start]
  while (queue.length) {
    const file = queue.shift()!
    for (const spec of staticSpecifiers(file, readFileSync(file, 'utf8'))) {
      const next = resolveLocal(src, spec, file)
      if (next && !seen.has(next)) {
        seen.add(next)
        queue.push(next)
      }
    }
  }
  return [...seen].map((f) => relative(src, f)).sort()
}

/** A declaration file's `@/…` specifiers, made relative to where the file
 *  sits under the types root. The host maps `@/` with a tsconfig path an app
 *  does not have — and must not have: an app that could name `@/server/…`
 *  would typecheck against host internals its build cannot import. `rel` is
 *  the declaration's path under the root that `@/` names. */
export function rewriteHostAliases(dts: string, rel: string): string {
  const fromDir = dirname(rel)
  const swap = (spec: string): string => {
    const target = spec.slice(2)
    let r = relative(fromDir, target)
    if (!r.startsWith('.')) r = `./${r}`
    return r
  }
  return dts
    .replace(/(\bfrom\s*['"])(@\/[^'"]+)(['"])/g, (_, a: string, spec: string, b: string) => `${a}${swap(spec)}${b}`)
    .replace(/(\bimport\(\s*['"])(@\/[^'"]+)(['"]\s*\))/g, (_, a: string, spec: string, b: string) => `${a}${swap(spec)}${b}`)
    .replace(/(\bimport\s*['"])(@\/[^'"]+)(['"])/g, (_, a: string, spec: string, b: string) => `${a}${swap(spec)}${b}`)
}

/** A digest of every source a kit is built from, so the dev server can tell a
 *  kit on disk is stale without building one to compare. `ui` is the ui
 *  root. Tests are left out: they ship in no kit. */
export function kitSourceDigest(ui: string): string {
  const src = join(ui, 'src')
  const h = createHash('sha256')
  const visit = (p: string) => {
    if (!existsSync(p)) return
    if (statSync(p).isDirectory()) {
      for (const n of readdirSync(p).sort()) visit(join(p, n))
    } else if (!p.endsWith('.test.ts')) {
      h.update(relative(ui, p))
      h.update('\0')
      h.update(readFileSync(p))
    }
  }
  for (const d of ['sdk', 'sdk-kit', 'server', 'app-runtime', 'lib', 'components/ui', 'styles.css']) visit(join(src, d))
  visit(join(ui, 'svelte.config.ts'))
  visit(join(ui, 'scripts/build-sdk-kit.ts'))
  visit(join(ui, '..', 'cli/src/cmd/app/skeleton.ts'))
  visit(join(ui, '..', 'docs/sdk/talaria.schema.json'))
  return h.digest('hex').slice(0, 16)
}

// ── tar ──────────────────────────────────────────────────────────────────────

export interface TarEntry {
  /** A path inside the archive, `/`-separated, no leading slash. */
  path: string
  data: string | Uint8Array
  /** 0o644 for a file, 0o755 for an executable. */
  mode?: number
}

const encoder = new TextEncoder()

function octal(n: number, width: number): string {
  return `${n.toString(8).padStart(width - 1, '0')}\0`
}

function header(path: string, size: number, mode: number): Uint8Array {
  const h = new Uint8Array(512)
  let name = path
  let prefix = ''
  if (encoder.encode(name).length > 100) {
    const cut = path.lastIndexOf('/', path.length - 1)
    // ustar: a path over 100 bytes splits at a slash into prefix (≤155) + name (≤100).
    let at = cut
    while (at > 0 && (encoder.encode(path.slice(at + 1)).length > 100 || encoder.encode(path.slice(0, at)).length > 155)) {
      at = path.lastIndexOf('/', at - 1)
    }
    if (at <= 0) throw new Error(`tar: path too long for ustar: ${path}`)
    prefix = path.slice(0, at)
    name = path.slice(at + 1)
  }
  const put = (s: string, off: number, len: number) => h.set(encoder.encode(s).subarray(0, len), off)
  put(name, 0, 100)
  put(octal(mode, 8), 100, 8)
  put(octal(0, 8), 108, 8) // uid
  put(octal(0, 8), 116, 8) // gid
  put(octal(size, 12), 124, 12)
  put(octal(0, 12), 136, 12) // mtime: fixed, so the same kit is the same bytes
  put('        ', 148, 8) // checksum placeholder
  put('0', 156, 1) // a regular file
  put('ustar\0', 257, 6)
  put('00', 263, 2)
  put(prefix, 345, 155)
  let sum = 0
  for (const b of h) sum += b
  put(`${sum.toString(8).padStart(6, '0')}\0 `, 148, 8)
  return h
}

/** A gzipped ustar archive of `entries`, in the order given. Directories are
 *  implied by the paths; every timestamp is zero, so the bytes depend on the
 *  content alone. */
export function tarGz(entries: readonly TarEntry[]): Buffer {
  const parts: Uint8Array[] = []
  for (const e of entries) {
    if (!e.path || e.path.startsWith('/') || e.path.split('/').includes('..')) throw new Error(`tar: refusing path ${JSON.stringify(e.path)}`)
    const data = typeof e.data === 'string' ? encoder.encode(e.data) : e.data
    parts.push(header(e.path, data.length, e.mode ?? 0o644))
    parts.push(data)
    const pad = (512 - (data.length % 512)) % 512
    if (pad) parts.push(new Uint8Array(pad))
  }
  parts.push(new Uint8Array(1024))
  return gzipSync(Buffer.concat(parts), { level: 9 })
}

// The SDK kit, as this host serves it (GET /api/sdk/kit.tgz). What the kit
// is: ui/src/sdk-kit/kit.ts. How it is built: ui/scripts/build-sdk-kit.ts.
//
// WHERE IT COMES FROM. A production image builds it with the ui
// (`bun run build`) and ships dist/sdk-kit beside the bundles; this host only
// reads it. A checkout running from source (`talaria dev`) builds it on the
// first request, and again whenever the sources it is built from change, so
// the kit a dev box serves is never older than its own SDK.
//
// WHO MAY HAVE IT. Anyone the instance knows: a signed-in member, or an
// enabled agent presenting its own credential (the org-wide key too, while
// that window is open). The kit carries no secret — declarations, the build
// recipe, the CLI — but it names this host's build and SDK version exactly,
// and nothing about it is for the open internet.
import { spawn } from 'node:child_process'
import { existsSync, readFileSync, statSync } from 'node:fs'
import { join } from 'node:path'
import { json } from './http'
import { uiRoot } from './app-build/paths'
import { KIT_PATHS, KIT_VERSION_HEADER } from '@/sdk-kit/kit'
import { kitSourceDigest } from '@/sdk-kit/pack'

/** dist/sdk-kit/kit.json: the kit's manifest, plus what the builder knows
 *  about the archive it wrote. */
export interface ServedKit {
  version: string
  hostBuild: string
  sdk: string
  digest: string
  bytes: number
  /** The sources digest it was built from (`kitSourceDigest`). */
  sources: string
}

const kitDir = () => join(uiRoot(), 'dist/sdk-kit')

export function readServedKit(dir: string = kitDir()): ServedKit | null {
  const meta = join(dir, 'kit.json')
  const tgz = join(dir, 'kit.tgz')
  if (!existsSync(meta) || !existsSync(tgz)) return null
  try {
    const m = JSON.parse(readFileSync(meta, 'utf8')) as Partial<ServedKit>
    if (typeof m.version !== 'string' || typeof m.hostBuild !== 'string' || typeof m.digest !== 'string' || typeof m.sdk !== 'string') return null
    if (statSync(tgz).size !== m.bytes) return null
    return { version: m.version, hostBuild: m.hostBuild, sdk: m.sdk, digest: m.digest, bytes: m.bytes, sources: typeof m.sources === 'string' ? m.sources : '' }
  } catch {
    return null
  }
}

/** The stamp a kit built now would carry: the client build's, when there is
 *  one — the builder reads the same file. */
function currentStamp(): string | null {
  const p = join(uiRoot(), KIT_PATHS.buildId)
  return existsSync(p) ? readFileSync(p, 'utf8').trim() || null : null
}

/** Running from source: the builder and the sources are here. */
export const buildsFromSource = (): boolean => existsSync(join(uiRoot(), 'scripts/build-sdk-kit.ts')) && existsSync(join(uiRoot(), 'src/sdk-kit'))

let building: Promise<string | null> | null = null

/** Run the builder once, however many requests are waiting on it. Null on
 *  success, else its last lines. */
function buildKit(): Promise<string | null> {
  if (building) return building
  building = new Promise<string | null>((resolveBuild) => {
    const child = spawn(process.env.TALARIA_BUN ?? 'bun', ['scripts/build-sdk-kit.ts'], { cwd: uiRoot(), stdio: ['ignore', 'pipe', 'pipe'] })
    let out = ''
    child.stdout.on('data', (d: Buffer) => (out += d.toString()))
    child.stderr.on('data', (d: Buffer) => (out += d.toString()))
    child.on('error', (e) => resolveBuild(`the kit builder could not start: ${e.message}`))
    child.on('close', (code) => resolveBuild(code === 0 ? null : out.trim().split('\n').slice(-6).join('\n') || `the kit builder exited ${code}`))
  }).finally(() => {
    building = null
  })
  return building
}

/** The kit to serve, building it first when this host runs from source and
 *  has none that matches its sources. */
export async function currentKit(): Promise<ServedKit | { error: string }> {
  const have = readServedKit()
  if (!buildsFromSource()) return have ?? { error: 'this build of Talaria ships no SDK kit (dist/sdk-kit is missing)' }
  const stamp = currentStamp()
  if (have && have.sources === kitSourceDigest(uiRoot()) && (!stamp || stamp === have.hostBuild)) return have
  const failed = await buildKit()
  if (failed) return { error: `the SDK kit could not be built: ${failed}` }
  return readServedKit() ?? { error: 'the SDK kit builder wrote no kit' }
}

/** GET / HEAD /api/sdk/kit.tgz, for a caller the route admitted. `edges` is
 *  the test seam: which kit, and the directory its archive is read from. */
export async function serveKit(
  request: Request,
  head = false,
  edges: { current?: () => Promise<ServedKit | { error: string }>; dir?: string } = {},
): Promise<Response> {
  const kit = await (edges.current ?? currentKit)()
  if ('error' in kit) return json({ error: kit.error }, { status: 503 })
  const etag = `"${kit.digest}"`
  const headers = new Headers({
    'content-type': 'application/gzip',
    'content-disposition': `attachment; filename="talaria-sdk-kit-${kit.version}.tgz"`,
    [KIT_VERSION_HEADER]: kit.version,
    'x-talaria-sdk-version': kit.sdk,
    etag,
    'cache-control': 'private, no-cache',
  })
  const match = request.headers.get('if-none-match')
  if (match && match.split(',').some((t) => t.trim() === etag)) return new Response(null, { status: 304, headers })
  headers.set('content-length', String(kit.bytes))
  if (head) return new Response(null, { status: 200, headers })
  return new Response(readFileSync(join(edges.dir ?? kitDir(), 'kit.tgz')), { status: 200, headers })
}

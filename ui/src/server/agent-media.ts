// Reading media out of agent containers — shared by the inline-image route
// and "save to artifacts". Guardrails live HERE so every consumer gets them:
// absolute paths inside /opt/data only (the agent's own volume, never the
// host), image types only, size-capped, slot-aware container resolution.
import { execFile } from 'node:child_process'
import { promisify } from 'node:util'
import { db } from './db/pg'
import { managedContainer } from './fleet-docker'

const run = promisify(execFile)

const IMAGE_TYPES: Record<string, string> = {
  png: 'image/png',
  jpg: 'image/jpeg',
  jpeg: 'image/jpeg',
  gif: 'image/gif',
  webp: 'image/webp',
}
const MAX_BYTES = 25 * 1024 * 1024

export type MediaError = { error: string; status: number }

/** Read one image from the agent's container. Returns bytes + mime, or a
 *  typed error the route can pass through. */
export async function readAgentImage(
  model: string,
  path: string,
): Promise<{ bytes: Uint8Array; mime: string } | MediaError> {
  if (!path.startsWith('/opt/data/') || path.includes('..') || path.includes('\0')) {
    return { error: 'only files under /opt/data', status: 400 }
  }
  const mime = IMAGE_TYPES[path.split('.').pop()?.toLowerCase() ?? '']
  if (!mime) return { error: 'images only (png/jpg/gif/webp)', status: 415 }

  const sql = await db()
  const rows = (await sql`
    select department from agent_defs where model = ${model} and managed
  `) as unknown as Array<{ department: string }>
  if (!rows[0]) return { error: 'unknown agent', status: 404 }

  try {
    const name = await managedContainer(rows[0].department)
    const { stdout } = await run('docker', ['exec', name, 'cat', path], {
      encoding: 'buffer',
      maxBuffer: MAX_BYTES,
      timeout: 30_000,
    })
    return { bytes: new Uint8Array(stdout), mime }
  } catch {
    return { error: 'file not found in the agent container', status: 404 }
  }
}

export const isMediaError = (r: { bytes: Uint8Array; mime: string } | MediaError): r is MediaError => 'error' in r

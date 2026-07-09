import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { execFile } from 'node:child_process'
import { promisify } from 'node:util'
import { getSessionUser } from '@/server/auth/session'
import { canUseAgentModel } from '@/server/users'
import { db } from '@/server/db/pg'
import { managedContainer } from '@/server/fleet-docker'

const run = promisify(execFile)

const IMAGE_TYPES: Record<string, string> = {
  png: 'image/png',
  jpg: 'image/jpeg',
  jpeg: 'image/jpeg',
  gif: 'image/gif',
  webp: 'image/webp',
}
const MAX_BYTES = 25 * 1024 * 1024

// GET ?path=/opt/data/… → stream an image out of the agent's container, so
// media agents produce ("MEDIA:<path>" in replies) renders inline in chat.
// Guardrails: the caller must be allowed to use the agent (same gate as
// chatting with it), only absolute paths inside /opt/data (the agent's own
// volume — never the host), images only, size-capped, nosniff.
export const Route = createFileRoute('/api/agent-media/$model')({
  server: {
    handlers: {
      GET: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (!(await canUseAgentModel(user.id, user.role, params.model))) {
          return json({ error: 'forbidden' }, { status: 403 })
        }

        const path = new URL(request.url).searchParams.get('path') ?? ''
        if (!path.startsWith('/opt/data/') || path.includes('..') || path.includes('\0')) {
          return json({ error: 'only files under /opt/data' }, { status: 400 })
        }
        const type = IMAGE_TYPES[path.split('.').pop()?.toLowerCase() ?? '']
        if (!type) return json({ error: 'images only (png/jpg/gif/webp)' }, { status: 415 })

        const sql = await db()
        const rows = (await sql`
          select department from agent_defs where model = ${params.model} and managed
        `) as unknown as Array<{ department: string }>
        if (!rows[0]) return json({ error: 'unknown agent' }, { status: 404 })

        try {
          const name = await managedContainer(rows[0].department)
          const { stdout } = await run('docker', ['exec', name, 'cat', path], {
            encoding: 'buffer',
            maxBuffer: MAX_BYTES,
            timeout: 30_000,
          })
          return new Response(new Uint8Array(stdout), {
            headers: {
              'content-type': type,
              'cache-control': 'private, max-age=300',
              'x-content-type-options': 'nosniff',
            },
          })
        } catch {
          return json({ error: 'file not found in the agent container' }, { status: 404 })
        }
      },
    },
  },
})

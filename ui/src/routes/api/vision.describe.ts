import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { parseBody, requireUser } from '@/server/api-guard'
import { agentCaller } from '@/server/agent-auth'
import { describeImage } from '@/server/vision'
import { canAccessUpload, getUpload } from '@/server/uploads'

const Body = z.object({
  uploadId: z.string().min(1).max(200),
  question: z.string().min(3).max(500),
})

// READ AN IMAGE ON BEHALF OF A MODEL THAT CANNOT — the endpoint behind the
// `describe_image` tool.
//
// THE ACCESS CHECK IS THE WHOLE SECURITY STORY HERE, and it is not this file's
// to invent: `readUploadForCaller` already answers "may this principal read this
// upload" for every attachment path, board policy included. An endpoint that
// resolved an upload id itself would be a second answer to that question, and
// the second answer is always the one that turns out to be wrong.
//
// WHAT COMES BACK IS ATTRIBUTED. The description carries the model that produced
// it, because the calling agent is about to treat it as fact and a surface that
// presents it as the caller's own observation is lying by omission.
export const Route = defineApi('/api/vision/describe', {
  POST: async ({ request }) => {
    const caller = await agentCaller(request)
    if (caller instanceof Response) return caller
    let viewer: Parameters<typeof canAccessUpload>[1]
    if (caller) {
      viewer = { agent: caller.model }
    } else {
      const gate = await requireUser(request)
      if (gate instanceof Response) return gate
      viewer = { userId: gate.id, who: gate.email ?? null, isAdmin: gate.role === 'admin' }
    }

    const body = await parseBody(request, Body)
    if (body instanceof Response) return body

    // ASKED BEFORE THE BYTES ARE FETCHED, and asked of the one function that
    // already answers it everywhere else. An endpoint that resolved an upload
    // itself would be a second answer to "may this principal read this", and the
    // second answer is always the one that turns out to be wrong.
    if (!(await canAccessUpload(body.uploadId, viewer).catch(() => false))) {
      return json({ error: 'no attachment with that id, or you are not allowed to read it' }, { status: 404 })
    }
    const file = await getUpload(body.uploadId).catch(() => null)
    if (!file) return json({ error: 'no attachment with that id, or you are not allowed to read it' }, { status: 404 })
    if (!file.mime.startsWith('image/')) {
      // Named rather than generic: the calling model has `fetch_attachment` for
      // this and the sentence is what tells it to use that instead.
      return json({ error: `that attachment is ${file.mime}, not an image — read it with fetch_attachment instead` }, { status: 400 })
    }

    const out = await describeImage({ image: `data:${file.mime};base64,${file.bytes.toString('base64')}`, question: body.question })
    if (out.error) return json({ error: out.error }, { status: 503 })
    return json({ description: out.text, model: out.model })
  },
})

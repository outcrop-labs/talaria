import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { Uuid } from '@/lib/api-schema'
import { parseBody, requireUser } from '@/server/api-guard'
import { acquireInboxFocusLock, runInboxConversationCommand } from '@/server/inbox-focus-conversation'

const Body = z.object({
  key: z.string().min(1).max(600).nullable().optional(),
  /** Which view the panel is open over. An ID the server maps to prose — see
   *  surfaceBrief — never the prose itself. */
  surface: z.string().max(40).nullable().optional(),
  instruction: z.string().trim().min(1).max(20_000),
  delegateModel: z.string().max(300).nullable().optional(),
  responseModel: z.string().max(300).nullable().optional(),
  mode: z.enum(['normal', 'fast', 'plan']).default('normal'),
  /** Which conversation instance (the panel's chat picker). Validated against
   *  the owner's live inbox conversations; stale ids fall back to their most
   *  recent instance rather than failing the command. */
  conversationId: Uuid.nullable().optional(),
  /** Reasoning effort for the reply, from the levels the answering model's
   *  metadata vouches for; omitted = the model's default. Validated server-side
   *  against the same metadata the composer's picker lists. */
  effort: z.string().max(24).nullable().optional(),
  attachmentIds: z.array(Uuid).max(12).default([]),
  refs: z.array(z.object({
    type: z.enum(['kb-doc', 'artifact']),
    id: Uuid,
  })).max(6).default([]),
})
// doc: Run one instruction from the focus inbox panel through the assistant
// doc: (normal / fast / plan mode, optional model overrides).


export const Route = defineApi('/api/inbox/focus/command', {
  POST: async ({ request }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    const body = await parseBody(request, Body)
    if (body instanceof Response) return body
    const release = acquireInboxFocusLock(user.id)
    if (!release) return json({ error: 'Your assistant is already handling another Inbox action.' }, { status: 409 })
    const encoder = new TextEncoder()
    const stream = new ReadableStream<Uint8Array>({
      async start(controller) {
        try {
          for await (const event of runInboxConversationCommand(user, {
            instruction: body.instruction,
            focusKey: body.key,
            surface: body.surface,
            delegateModel: body.delegateModel,
            responseModel: body.responseModel,
            mode: body.mode,
            conversationId: body.conversationId ?? null,
            effort: body.effort ?? null,
            attachmentIds: body.attachmentIds,
            refs: body.refs,
            signal: request.signal,
          })) {
            controller.enqueue(encoder.encode(`event: ${event.type}\ndata: ${JSON.stringify(event)}\n\n`))
          }
        } catch (error) {
          if (!request.signal.aborted) {
            const message = error instanceof Error ? error.message : 'Your assistant could not start that response.'
            controller.enqueue(encoder.encode(`event: error\ndata: ${JSON.stringify({ type: 'error', message })}\n\n`))
          }
        } finally {
          release()
          try { controller.close() } catch { /* the client already disconnected */ }
        }
      },
    })
    return new Response(stream, {
      headers: {
        'Content-Type': 'text/event-stream; charset=utf-8',
        'Cache-Control': 'no-cache, no-transform',
        Connection: 'keep-alive',
      },
    })
  },
})

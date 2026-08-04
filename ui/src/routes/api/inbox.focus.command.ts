import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { parseBody, requireUser } from '@/server/api-guard'
import { acquireInboxFocusLock, runInboxConversationCommand } from '@/server/inbox-focus-conversation'

const Body = z.object({
  key: z.string().min(1).max(600).nullable().optional(),
  instruction: z.string().trim().min(1).max(20_000),
  delegateModel: z.string().max(300).nullable().optional(),
  responseModel: z.string().max(300).nullable().optional(),
  mode: z.enum(['normal', 'fast', 'plan']).default('normal'),
  attachmentIds: z.array(z.string().uuid()).max(12).default([]),
  refs: z.array(z.object({
    type: z.enum(['kb-doc', 'artifact']),
    id: z.string().uuid(),
  })).max(6).default([]),
})

export const Route = createFileRoute('/api/inbox/focus/command')({
  server: {
    handlers: {
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
                delegateModel: body.delegateModel,
                responseModel: body.responseModel,
                mode: body.mode,
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
    },
  },
})

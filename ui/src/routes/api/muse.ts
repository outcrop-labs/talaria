import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { parseBody, requireUser } from '@/server/api-guard'
import { buildMuseMessages, museModelFor, type MuseKind } from '@/server/muse'
import { buildUpstream, fetchUpstream, recordGatewayUsage, resolveRoute } from '@/server/llm-gateway'
import { estimateTokens } from '@/server/usage'

const Body = z.object({
  kind: z.enum(['soul', 'personality', 'skill', 'memory', 'cron', 'agent', 'document', 'template']),
  instruction: z.string().trim().min(1).max(8_000),
  current: z.string().max(300_000).optional(),
  context: z.string().max(2_000).optional(),
  chat: z
    .array(z.object({ role: z.enum(['user', 'assistant']), content: z.string().max(300_000) }))
    .max(24)
    .optional(),
})

// POST → stream a drafted document (or cron JSON) as plain text chunks.
// Runs on the caller's preferred model via the gateway machinery, metered as
// `muse:<user>`. Any signed-in user; what they can DO with the draft is
// still governed by the save endpoints' own authorization.
export const Route = createFileRoute('/api/muse')({
  server: {
    handlers: {
      POST: async ({ request }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        const body = await parseBody(request, Body)
        if (body instanceof Response) return body

        const model = await museModelFor(user.id)
        if (!model) return json({ error: 'no routable model found — add an endpoint with models on /models first' }, { status: 400 })
        const route = await resolveRoute(model)
        if (!route) return json({ error: `model "${model}" is not routable` }, { status: 400 })

        const messages = await buildMuseMessages({ ...body, kind: body.kind as MuseKind })
        let upstream
        try {
          upstream = await buildUpstream(route, { model, messages, stream: true, temperature: 0.4 })
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 502 })
        }

        const res = await fetchUpstream(upstream, route).catch((e: Error) => e)
        if (res instanceof Error) return json({ error: `upstream unreachable: ${res.message}` }, { status: 502 })
        if (!res.ok || !res.body) {
          const text = await res.text().catch(() => '')
          return json({ error: `model error (${res.status}): ${text.slice(0, 300)}` }, { status: 502 })
        }

        // Re-emit the SSE stream as bare text chunks (simple client), scanning
        // for usage to meter; estimates if the upstream doesn't report usage.
        const promptChars = messages.reduce((n, m) => n + m.content.length, 0)
        let lineBuf = ''
        let contentChars = 0
        let usage: { prompt_tokens?: number; completion_tokens?: number } | null = null
        const decoder = new TextDecoder()
        const encoder = new TextEncoder()

        const transform = new TransformStream<Uint8Array, Uint8Array>({
          transform(chunk, controller) {
            lineBuf += decoder.decode(chunk, { stream: true })
            const lines = lineBuf.split('\n')
            lineBuf = lines.pop() ?? ''
            for (const line of lines) {
              const data = line.startsWith('data:') ? line.slice(5).trim() : null
              if (!data || data === '[DONE]') continue
              try {
                const j = JSON.parse(data) as {
                  usage?: { prompt_tokens?: number; completion_tokens?: number } | null
                  choices?: Array<{ delta?: { content?: string } }>
                }
                if (j.usage) usage = j.usage
                const piece = j.choices?.[0]?.delta?.content
                if (piece) {
                  contentChars += piece.length
                  controller.enqueue(encoder.encode(piece))
                }
              } catch {
                /* partial line — wait for more */
              }
            }
          },
          flush() {
            void recordGatewayUsage({
              caller: `platform:muse:${user.email ?? user.name ?? user.id}`,
              endpoint: route.endpoint,
              upstreamModel: route.upstreamModel,
              promptTokens: usage?.prompt_tokens ?? estimateTokens(promptChars),
              completionTokens: usage?.completion_tokens ?? estimateTokens(contentChars),
              estimated: !usage,
            }).catch(() => {})
          },
        })

        return new Response(res.body.pipeThrough(transform), {
          status: 200,
          headers: {
            'content-type': 'text/plain; charset=utf-8',
            'cache-control': 'no-cache',
            'x-muse-model': model,
          },
        })
      },
    },
  },
})

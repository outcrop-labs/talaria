import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { authenticateKey } from '@/server/llm-keys'
import { buildUpstream, fetchUpstream, recordGatewayUsage, resolveRoute } from '@/server/llm-gateway'
import { estimateTokens } from '@/server/usage'
import { guardCompletion } from '@/server/guardrails'

// OpenAI-compatible chat completions over the org's model stack. Streaming and
// non-streaming both pass through; every call is metered into the ledger with
// the calling key's identity, priced like everything else.
export const Route = createFileRoute('/api/llm/v1/chat/completions')({
  server: {
    handlers: {
      POST: async ({ request }) => {
        const bearer = request.headers.get('authorization')?.replace(/^Bearer\s+/i, '') ?? null
        const id = await authenticateKey(bearer)
        if (!id) return json({ error: { message: 'invalid API key' } }, { status: 401 })

        const body = (await request.json().catch(() => null)) as Record<string, unknown> | null
        if (!body || typeof body.model !== 'string' || !Array.isArray(body.messages)) {
          return json({ error: { message: 'model and messages are required' } }, { status: 400 })
        }
        const route = await resolveRoute(body.model)
        if (!route) return json({ error: { message: `unknown model "${body.model}" — GET /v1/models` } }, { status: 404 })

        let upstream: { url: string; headers: Record<string, string>; body: Record<string, unknown> }
        try {
          upstream = await buildUpstream(route, body)
        } catch (e) {
          return json({ error: { message: (e as Error).message } }, { status: 502 })
        }

        const caller = `api:${id.keyName}`
        const promptChars = (body.messages as Array<{ content?: unknown }>).reduce(
          (n, m) => n + (typeof m.content === 'string' ? m.content.length : JSON.stringify(m.content ?? '').length),
          0,
        )

        const res = await fetchUpstream(upstream).catch((e: Error) => e)
        if (res instanceof Error) return json({ error: { message: `upstream unreachable: ${res.message}` } }, { status: 502 })

        const ledger = (usage: { prompt_tokens?: number; completion_tokens?: number } | null, contentChars: number) =>
          recordGatewayUsage({
            caller,
            endpoint: route.endpoint,
            upstreamModel: route.upstreamModel,
            promptTokens: usage?.prompt_tokens ?? estimateTokens(promptChars),
            completionTokens: usage?.completion_tokens ?? estimateTokens(contentChars),
            estimated: !usage,
          }).catch(() => {})

        // ── Non-streaming: read, meter, relay ─────────────────────────────────
        if (!upstream.body.stream) {
          const text = await res.text()
          if (res.ok) {
            try {
              const j = JSON.parse(text) as {
                usage?: { prompt_tokens?: number; completion_tokens?: number }
                choices?: Array<{ message?: { content?: string } }>
              }
              const content = j.choices?.[0]?.message?.content ?? ''
              void ledger(j.usage ?? null, content.length)
              void guardCompletion({ answer: content, messages: body.messages as unknown[], caller, model: body.model as string, endpoint: route.endpoint.name }).catch(() => {})
            } catch {
              /* relay verbatim even if unparseable */
            }
          }
          return new Response(text, {
            status: res.status,
            headers: { 'content-type': res.headers.get('content-type') ?? 'application/json' },
          })
        }

        // ── Streaming: pass bytes through, scan SSE lines for usage/content ──
        if (!res.ok || !res.body) {
          const text = await res.text().catch(() => '')
          return new Response(text || JSON.stringify({ error: { message: `upstream ${res.status}` } }), {
            status: res.status,
            headers: { 'content-type': res.headers.get('content-type') ?? 'application/json' },
          })
        }

        let lineBuf = ''
        let content = ''
        let usage: { prompt_tokens?: number; completion_tokens?: number } | null = null
        const decoder = new TextDecoder()
        const scan = (chunkText: string) => {
          lineBuf += chunkText
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
              content += j.choices?.[0]?.delta?.content ?? ''
            } catch {
              /* partial or non-JSON line — ignore */
            }
          }
        }

        const meter = new TransformStream<Uint8Array, Uint8Array>({
          transform(chunk, controller) {
            controller.enqueue(chunk)
            scan(decoder.decode(chunk, { stream: true }))
          },
          flush() {
            void ledger(usage, content.length)
            // Confab guard runs on the assembled text after the stream — never
            // delays streaming; observe mode records findings out-of-band.
            void guardCompletion({ answer: content, messages: body.messages as unknown[], caller, model: body.model as string, endpoint: route.endpoint.name }).catch(() => {})
          },
        })

        return new Response(res.body.pipeThrough(meter), {
          status: 200,
          headers: {
            'content-type': res.headers.get('content-type') ?? 'text/event-stream',
            'cache-control': 'no-cache',
          },
        })
      },
    },
  },
})

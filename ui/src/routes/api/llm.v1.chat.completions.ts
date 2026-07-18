import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { authenticateKey } from '@/server/llm-keys'
import { buildUpstream, fetchUpstream, recordGatewayUsage, resolveRoute } from '@/server/llm-gateway'
import { estimateTokens } from '@/server/usage'
import { getGuardConfig, guardCompletion, needsRedaction, redactSecrets } from '@/server/guardrails'
import { getSetting } from '@/server/audit'

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
        // Keys whose LLM usage is metered DOWNSTREAM (e.g. the fleet key — agents
        // route their tool loop through here, but their chat/channel/ticket usage
        // is already metered by those flows). Skip gateway metering to avoid a
        // double-count; the guard still runs.
        const unmetered = await getSetting<string[]>('gateway_unmetered_keys', ['fleet-gateway'])
        const skipMeter = unmetered.includes(id.keyName)
        // The same keys mark agent INNER LOOPS (the fleet routes agents' tool
        // loops through here). Guard caveats are for humans reading a reply —
        // injecting one into an agent's own loop would contaminate its context,
        // so annotate/strict never touch these keys' responses (findings still
        // record, and the chat/channel layer annotates the human-facing copy).
        const guardCfg = await getGuardConfig()
        const mayAnnotate = !skipMeter && (guardCfg.mode === 'annotate' || guardCfg.mode === 'strict')
        const promptChars = (body.messages as Array<{ content?: unknown }>).reduce(
          (n, m) => n + (typeof m.content === 'string' ? m.content.length : JSON.stringify(m.content ?? '').length),
          0,
        )

        const res = await fetchUpstream(upstream, route).catch((e: Error) => e)
        if (res instanceof Error) return json({ error: { message: `upstream unreachable: ${res.message}` } }, { status: 502 })

        const ledger = (usage: { prompt_tokens?: number; completion_tokens?: number } | null, contentChars: number) => {
          if (skipMeter) return
          return recordGatewayUsage({
            caller,
            endpoint: route.endpoint,
            upstreamModel: route.upstreamModel,
            promptTokens: usage?.prompt_tokens ?? estimateTokens(promptChars),
            completionTokens: usage?.completion_tokens ?? estimateTokens(contentChars),
            estimated: !usage,
          }).catch(() => {})
        }

        // ── Non-streaming: read, meter, relay ─────────────────────────────────
        if (!upstream.body.stream) {
          let text = await res.text()
          if (res.ok) {
            try {
              const j = JSON.parse(text) as {
                usage?: { prompt_tokens?: number; completion_tokens?: number }
                choices?: Array<{ message?: { content?: string } }>
              }
              const content = j.choices?.[0]?.message?.content ?? ''
              void ledger(j.usage ?? null, content.length)
              if (mayAnnotate && content) {
                // Nothing has been relayed yet, so annotate/strict can act on
                // the body itself: strict redacts leaked secrets, and any
                // findings append the human-facing caveat.
                const g = await guardCompletion({ answer: content, messages: body.messages as unknown[], caller, model: body.model as string, endpoint: route.endpoint.name })
                if (g.findings.length && j.choices?.[0]?.message) {
                  const safe = g.mode === 'strict' && needsRedaction(g.findings) ? redactSecrets(content).text : content
                  j.choices[0].message.content = `${safe}${g.caveat}`
                  text = JSON.stringify(j)
                }
              } else {
                void guardCompletion({ answer: content, messages: body.messages as unknown[], caller, model: body.model as string, endpoint: route.endpoint.name }).catch(() => {})
              }
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
        const encoder = new TextEncoder()
        const isDone = (line: string) => line.startsWith('data:') && line.slice(5).trim() === '[DONE]'
        const scanLine = (line: string) => {
          const data = line.startsWith('data:') ? line.slice(5).trim() : null
          if (!data || data === '[DONE]') return
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
        const splitLines = (chunkText: string) => {
          lineBuf += chunkText
          const lines = lineBuf.split('\n')
          lineBuf = lines.pop() ?? ''
          return lines
        }

        // Passthrough meter: bytes relay untouched; the guard runs on the
        // assembled text after the stream (observe posture, or an agent-loop
        // key that must never see a caveat) — never delays streaming.
        const meter = new TransformStream<Uint8Array, Uint8Array>({
          transform(chunk, controller) {
            controller.enqueue(chunk)
            for (const line of splitLines(decoder.decode(chunk, { stream: true }))) scanLine(line)
          },
          flush() {
            if (lineBuf) scanLine(lineBuf)
            void ledger(usage, content.length)
            void guardCompletion({ answer: content, messages: body.messages as unknown[], caller, model: body.model as string, endpoint: route.endpoint.name }).catch(() => {})
          },
        })

        // Annotate meter: relay line-wise but withhold [DONE]; once upstream
        // ends, run the guard and inject any caveat as one final delta chunk
        // before releasing [DONE]. Streamed bytes are already gone, so strict
        // can't redact here — the caveat still lands.
        let sawDone = false
        const annotateMeter = new TransformStream<Uint8Array, Uint8Array>({
          transform(chunk, controller) {
            for (const line of splitLines(decoder.decode(chunk, { stream: true }))) {
              scanLine(line)
              if (isDone(line)) sawDone = true
              else controller.enqueue(encoder.encode(`${line}\n`))
            }
          },
          async flush(controller) {
            if (lineBuf) {
              scanLine(lineBuf)
              if (isDone(lineBuf)) sawDone = true
              else controller.enqueue(encoder.encode(lineBuf))
            }
            void ledger(usage, content.length)
            const g = content
              ? await guardCompletion({ answer: content, messages: body.messages as unknown[], caller, model: body.model as string, endpoint: route.endpoint.name }).catch(() => null)
              : null
            if (g?.caveat) {
              const chunk = { id: 'talaria-guard', object: 'chat.completion.chunk', model: body.model, choices: [{ index: 0, delta: { content: g.caveat }, finish_reason: null }] }
              controller.enqueue(encoder.encode(`data: ${JSON.stringify(chunk)}\n\n`))
            }
            if (sawDone) controller.enqueue(encoder.encode('data: [DONE]\n\n'))
          },
        })

        return new Response(res.body.pipeThrough(mayAnnotate ? annotateMeter : meter), {
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

import { parseAgentStream } from '@/lib/sse-parse'
import { proxyChat } from './gateway'

function parseJsonObject(text: string): unknown {
  const start = text.indexOf('{')
  const end = text.lastIndexOf('}')
  if (start === -1 || end <= start) return null
  try {
    return JSON.parse(text.slice(start, end + 1))
  } catch {
    return null
  }
}

async function streamText(body: ReadableStream<Uint8Array>, max: number, signal: AbortSignal): Promise<string> {
  let text = ''
  const cancel = () => { void body.cancel(signal.reason).catch(() => {}) }
  signal.addEventListener('abort', cancel, { once: true })
  try {
    for await (const event of parseAgentStream(body)) {
      signal.throwIfAborted()
      if (event.type === 'content' && text.length < max) text += event.text.slice(0, max - text.length)
    }
  } finally {
    signal.removeEventListener('abort', cancel)
  }
  return text.trim()
}

function deadlineSignal(parent?: AbortSignal): { signal: AbortSignal; dispose: () => void } {
  const controller = new AbortController()
  const timeout = setTimeout(() => controller.abort(new DOMException('Inbox assistant timed out', 'TimeoutError')), 10_000)
  const abortFromParent = () => controller.abort(parent?.reason)
  parent?.addEventListener('abort', abortFromParent, { once: true })
  return {
    signal: controller.signal,
    dispose: () => {
      clearTimeout(timeout)
      parent?.removeEventListener('abort', abortFromParent)
    },
  }
}

export async function requestText(
  model: string,
  messages: Array<{ role: 'system' | 'user' | 'assistant'; content: string }>,
  max = 20_000,
  parentSignal?: AbortSignal,
): Promise<string | null> {
  const deadline = deadlineSignal(parentSignal)
  try {
    const upstream = await proxyChat(
      {
        model,
        messages,
        tools: [],
        tool_choice: 'none',
        temperature: 0.2,
      },
      { waitMs: 10_000, signal: deadline.signal },
    ).catch(() => null)
    if (!upstream?.ok || !upstream.body) return null
    return (await streamText(upstream.body, max, deadline.signal)) || null
  } finally {
    deadline.dispose()
  }
}

export async function requestJsonObject(model: string, prompt: string, max = 6_000, parentSignal?: AbortSignal): Promise<unknown> {
  const deadline = deadlineSignal(parentSignal)
  try {
    const upstream = await proxyChat(
      {
        model,
        messages: [{ role: 'user', content: prompt }],
        tools: [],
        tool_choice: 'none',
        response_format: { type: 'json_object' },
        temperature: 0.1,
      },
      { waitMs: 10_000, signal: deadline.signal },
    ).catch(() => null)
    if (!upstream?.ok || !upstream.body) return null
    return parseJsonObject(await streamText(upstream.body, max, deadline.signal))
  } finally {
    deadline.dispose()
  }
}

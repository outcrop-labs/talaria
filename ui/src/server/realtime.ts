// Real-time fan-out over Redis pub/sub → SSE. Mutations publish a small event
// to a topic (`board:<id>`, `channel:<id>`); each connected client holds an SSE
// stream fed by a dedicated Redis subscriber. Multiplayer without websockets.
import Redis from 'ioredis'
import { getRedis } from './db/redis'

export interface BoardEvent {
  type: 'task' | 'comment' | 'board'
  taskId?: string
  deleted?: boolean
}

export function publishBoard(boardId: string, event: BoardEvent): void {
  void getRedis().publish(`board:${boardId}`, JSON.stringify(event))
}

export interface ChannelEvent {
  type: 'message' | 'channel'
  messageId?: string
  seq?: number
  deleted?: boolean
}

export function publishChannel(channelId: string, event: ChannelEvent): void {
  void getRedis().publish(`channel:${channelId}`, JSON.stringify(event))
}

/** An SSE ReadableStream of a board's events (own Redis subscriber per client). */
export function boardEventStream(boardId: string, signal: AbortSignal): ReadableStream<Uint8Array> {
  return topicEventStream(`board:${boardId}`, signal)
}

/** An SSE ReadableStream of a chat channel's events. */
export function channelEventStream(channelId: string, signal: AbortSignal): ReadableStream<Uint8Array> {
  return topicEventStream(`channel:${channelId}`, signal)
}

function topicEventStream(channel: string, signal: AbortSignal): ReadableStream<Uint8Array> {
  const enc = new TextEncoder()
  const sub = new Redis(process.env.REDIS_URL!, { maxRetriesPerRequest: 3 })

  return new ReadableStream<Uint8Array>({
    start(controller) {
      const send = (s: string) => {
        try {
          controller.enqueue(enc.encode(s))
        } catch {
          /* stream closed */
        }
      }
      send(': connected\n\n')
      // Swallow connect/subscribe failures (e.g. the client aborts before the
      // subscriber finishes connecting) — an unhandled rejection or 'error'
      // event here would take down the whole server process.
      sub.on('error', () => {})
      sub.subscribe(channel).catch(() => {})
      sub.on('message', (_ch, msg) => send(`data: ${msg}\n\n`))
      const ping = setInterval(() => send(': ping\n\n'), 25_000)

      const cleanup = () => {
        clearInterval(ping)
        sub.disconnect()
        try {
          controller.close()
        } catch {
          /* already closed */
        }
      }
      if (signal.aborted) cleanup()
      else signal.addEventListener('abort', cleanup, { once: true })
    },
    cancel() {
      sub.disconnect()
    },
  })
}

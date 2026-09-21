// The one SSE door.
//
// Four surfaces open an EventSource: the board rail (invalidate on any board
// event), the channel rail (parse the frame, invalidate what it names), the
// account's `/api/me/events` fan (one shared subscription, reference-counted),
// and the work-watch panel (parse phases and the terminal state). Each grew its
// own `new EventSource(...)`, its own `onmessage`, and its own teardown — and
// the one thing they disagreed about is the part that matters when it breaks:
// what happens to a frame that does not parse. Two swallow it silently, one
// lets JSON.parse throw inside the listener (which the browser reports as an
// unhandled error and keeps the stream open), and the newest returns early.
//
// The door does not decide any of that: it hands the RAW data over and lets the
// caller parse, because "a frame I cannot parse is a frame I ignore" is a
// per-surface judgment (every consumer here has a poll floor behind it). What
// the door owns is the connection: one EventSource, one `onmessage` wiring, and
// a close that cannot be forgotten.

/** Subscribe to an SSE endpoint. Returns the unsubscribe — call it from the
 *  effect's teardown, where the four call sites used to write `es.close()`. */
export function openStream(url: string, onMessage: (data: string) => void): () => void {
  const es = new EventSource(url)
  es.onmessage = (e: MessageEvent<string>) => onMessage(e.data)
  return () => es.close()
}

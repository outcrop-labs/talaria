import { json } from '@/server/http'

// Map a Google service error onto a consistent API response. `surface` names the
// product (Drive, Calendar, Gmail) for the reconnect/unavailable messages.
export function googleFail(err: Error, surface: string): Response {
  if (err.name === 'GoogleNotConnected') {
    return json({ error: 'not_connected', message: 'Connect a Google account first.' }, { status: 409 })
  }
  if (/insufficient|ACCESS_TOKEN_SCOPE/i.test(err.message)) {
    return json({ error: 'reconnect_needed', message: `Reconnect Google to grant ${surface} access.` }, { status: 409 })
  }
  if (import.meta.env.DEV) console.error(`[google/${surface.toLowerCase()}] failed:`, err)
  return json({ error: 'google_error', message: `Could not reach Google ${surface}.` }, { status: 502 })
}

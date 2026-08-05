// The one HTTP helper every API route uses. Formerly `json` from
// '@tanstack/react-start'; same signature, no framework underneath.
export function json(data: unknown, init?: ResponseInit): Response {
  const headers = new Headers(init?.headers)
  if (!headers.has('content-type')) headers.set('content-type', 'application/json')
  return new Response(JSON.stringify(data), { ...init, headers })
}

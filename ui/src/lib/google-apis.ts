// The Google Cloud APIs the workspace integration calls, and where each is
// enabled. Client-safe by design — the Admin UI renders the list as setup
// instructions, the server probes them (server/google/api-health.ts imports
// from here, never the other way: @/server must not reach a browser bundle).

export const GOOGLE_API_LIBRARY = [
  { service: 'drive', name: 'Google Drive API', consoleUrl: 'https://console.cloud.google.com/apis/library/drive.googleapis.com' },
  { service: 'calendar', name: 'Google Calendar API', consoleUrl: 'https://console.cloud.google.com/apis/library/calendar-json.googleapis.com' },
  { service: 'gmail', name: 'Gmail API', consoleUrl: 'https://console.cloud.google.com/apis/library/gmail.googleapis.com' },
] as const

export type GoogleApiService = (typeof GOOGLE_API_LIBRARY)[number]['service']

export interface GoogleApiHealth {
  service: GoogleApiService
  name: string
  consoleUrl: string
  /** ok · disabled (enable it in the console) · error (anything else) */
  state: 'ok' | 'disabled' | 'error'
  /** Short human sentence for the failing states; null when ok. */
  detail: string | null
}

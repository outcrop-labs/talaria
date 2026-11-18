// The Google Cloud APIs the workspace integration calls, and where each is
// enabled. Client-safe by design — the Admin UI renders the list as setup
// instructions, the Rust api probes them (api/crates/talaria-google-api-health
// keeps a mirror of this list — the api cannot import a browser module).

export const GOOGLE_API_LIBRARY = [
  { service: 'drive', name: 'Google Drive API', consoleUrl: 'https://console.cloud.google.com/apis/library/drive.googleapis.com' },
  { service: 'calendar', name: 'Google Calendar API', consoleUrl: 'https://console.cloud.google.com/apis/library/calendar-json.googleapis.com' },
  { service: 'gmail', name: 'Gmail API', consoleUrl: 'https://console.cloud.google.com/apis/library/gmail.googleapis.com' },
  { service: 'sheets', name: 'Google Sheets API', consoleUrl: 'https://console.cloud.google.com/apis/library/sheets.googleapis.com' },
  { service: 'slides', name: 'Google Slides API', consoleUrl: 'https://console.cloud.google.com/apis/library/slides.googleapis.com' },
] as const

export type GoogleApiService = (typeof GOOGLE_API_LIBRARY)[number]['service']

export interface GoogleApiHealth {
  service: GoogleApiService
  name: string
  /** Where to enable it: the project's own activation page when Google named
   *  one in a disabled answer, else the library page. */
  consoleUrl: string
  /** ok · disabled (enable it in the console) · error (anything else) */
  state: 'ok' | 'disabled' | 'error'
  /** Google's own sentence for the failing states; null when ok. */
  detail: string | null
}

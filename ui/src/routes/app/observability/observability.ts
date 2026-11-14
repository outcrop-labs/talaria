// Observability — the ops plane in one place. The root is an OVERVIEW (a
// cross-section of alerts, live compute, spend, and the audit pulse); the
// tabs are the detail views the old standalone pages became.
export const OBS_TABS = [
  { id: 'overview', label: 'Overview' },
  { id: 'compute', label: 'Compute' },
  { id: 'host', label: 'Host' },
  { id: 'cost', label: 'Cost' },
  { id: 'audit', label: 'Audit' },
  // The MCP gateway's call record: which agent called which tool, and how it
  // went. The URL segment stays `mcp`.
  { id: 'mcp', label: 'Tool calls' },
  { id: 'alerts', label: 'Alerts' },
] as const
export type ObsTab = (typeof OBS_TABS)[number]['id']

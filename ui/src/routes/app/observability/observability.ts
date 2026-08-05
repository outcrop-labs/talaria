// Observability — the ops plane in one place. The root is an OVERVIEW (a
// cross-section of alerts, live compute, spend, and the audit pulse); the
// tabs are the detail views the old standalone pages became.
export const OBS_TABS = [
  { id: 'overview', label: 'Overview' },
  { id: 'compute', label: 'Compute' },
  { id: 'cost', label: 'Cost' },
  { id: 'audit', label: 'Audit' },
  { id: 'alerts', label: 'Alerts' },
] as const
export type ObsTab = (typeof OBS_TABS)[number]['id']

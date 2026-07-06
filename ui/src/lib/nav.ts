// The application menu — two mental modes. WORK is where everyone gets things
// done (chat, channels, boards, inbox); MANAGE is the control plane for the
// people running the platform (fleet, models, compute, cost, audit, admin).
// Simple for the non-technical surfaces, full control for the technical ones.

export interface NavItem {
  to: string
  label: string
  icon: string
  adminOnly?: boolean
}

// Views an admin can grant/revoke per user. Home and Settings are always
// reachable; admin-only views are gated by role already.
export const GATEABLE_VIEWS: { to: string; label: string }[] = [
  { to: '/chat', label: 'Chat' },
  { to: '/channels', label: 'Channels' },
  { to: '/boards', label: 'Boards' },
  { to: '/inbox', label: 'Inbox' },
  { to: '/agents', label: 'Agents' },
  { to: '/inference', label: 'Compute' },
  { to: '/cost', label: 'Cost' },
  { to: '/activity', label: 'Audit' },
  { to: '/alerts', label: 'Alerts' },
]

export interface NavSection {
  title: string
  items: NavItem[]
}

export const NAV: NavSection[] = [
  {
    title: 'Work',
    items: [
      { to: '/', label: 'Home', icon: '◇' },
      { to: '/chat', label: 'Chat', icon: '◈' },
      { to: '/channels', label: 'Channels', icon: '⋕' },
      { to: '/boards', label: 'Boards', icon: '⧉' },
      { to: '/inbox', label: 'Inbox', icon: '⌾' },
    ],
  },
  {
    title: 'Manage',
    items: [
      { to: '/agents', label: 'Agents', icon: '◍' },
      { to: '/models', label: 'Models', icon: '▤', adminOnly: true },
      { to: '/inference', label: 'Compute', icon: '▚' },
      { to: '/cost', label: 'Cost', icon: '⌗' },
      { to: '/activity', label: 'Audit', icon: '⌁' },
      { to: '/alerts', label: 'Alerts', icon: '△' },
    ],
  },
  {
    title: 'System',
    items: [
      { to: '/settings', label: 'Settings', icon: '⚙' },
      { to: '/admin', label: 'Admin', icon: '⛨', adminOnly: true },
    ],
  },
]

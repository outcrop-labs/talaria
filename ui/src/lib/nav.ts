// The main application menu — grouped nav for every surface Talaria is building
// toward feature parity (chat + conductor/swarm from hermes-workspace; fleet/
// tasks/cost/activity from mission-control; agent internals; system + admin).

export interface NavItem {
  to: string
  label: string
  icon: string
  adminOnly?: boolean
}

export interface NavSection {
  title: string
  items: NavItem[]
}

export const NAV: NavSection[] = [
  {
    title: 'Workspace',
    items: [
      { to: '/', label: 'Chat', icon: '◈' },
      { to: '/channels', label: 'Channels', icon: '⋕' },
      { to: '/inbox', label: 'Inbox', icon: '⌾' },
      { to: '/boards', label: 'Boards', icon: '⧉' },
    ],
  },
  {
    title: 'Fleet',
    items: [
      { to: '/fleet', label: 'Overview', icon: '⬡' },
      { to: '/agents', label: 'Agents', icon: '◍' },
      { to: '/tasks', label: 'Tasks', icon: '☰' },
      { to: '/cost', label: 'Cost', icon: '⌗' },
      { to: '/activity', label: 'Activity', icon: '⌁' },
      { to: '/alerts', label: 'Alerts', icon: '△' },
    ],
  },
  {
    title: 'Agent',
    items: [
      { to: '/skills', label: 'Skills', icon: '✦' },
      { to: '/memory', label: 'Memory', icon: '❖' },
      { to: '/mcp', label: 'MCP', icon: '⧈' },
    ],
  },
  {
    title: 'System',
    items: [
      { to: '/models', label: 'Models', icon: '▤', adminOnly: true },
      { to: '/inference', label: 'Inference', icon: '▚' },
      { to: '/settings', label: 'Settings', icon: '⚙' },
      { to: '/admin', label: 'Admin', icon: '⛨', adminOnly: true },
    ],
  },
]

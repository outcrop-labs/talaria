// The application menu. Views speak for themselves: the Work section has no
// header above it (the views are self-explanatory), Manage keeps one because
// it is the control plane. The top strip's System breadcrumb for
// settings/admin is TopStrip's own doctrine and lives there. Sections carry
// stable `id`s that consumers branch on — Apps slotting, app-manage slotting;
// `title` is display-only and optional, and a section without one renders as
// a bare list (NavRail).

import type { LucideIcon } from '@lucide/svelte'
import {
  Activity,
  BookOpen,
  Bot,
  CalendarRange,
  Cpu,
  FileBox,
  Hexagon,
  Inbox,
  LayoutGrid,
  LayoutTemplate,
  MessageCircle,
  PlugZap,
  Settings2,
  Telescope,
  Users,
} from '@lucide/svelte'

export interface NavItem {
  to: string
  label: string
  /** Lucide icon component for core items; enabled apps inject string glyphs
   *  (their manifest `icon`) — renderers must handle both. */
  icon: LucideIcon | string
  adminOnly?: boolean
  /** Which rail badge this item carries, if any — the /api/unreads arm its
   *  count comes from. `/home` deliberately has none here: its badge is the
   *  inbox-focus plane, a different query with its own doctrine. */
  badge?: 'comms' | 'plan' | 'research'
}

// Views an admin can grant/revoke per user. Home and Settings are always
// reachable. Work views default ALLOWED (denials stored per user); Manage
// views default DENIED for members (explicit allows stored per user) — same
// checklist in Admin → People, opposite resting state.
export const GATEABLE_VIEWS: { to: string; label: string }[] = [
  { to: '/comms', label: 'Comms' },
  { to: '/plan', label: 'Plan' },
  { to: '/boards', label: 'Boards' },
  { to: '/research', label: 'Research' },
  { to: '/knowledge', label: 'Knowledge' },
  { to: '/artifacts', label: 'Files' },
]

/** Manage-section views a member can be granted (pairs with the fine-grained
 *  permissions: view access opens the door, permissions gate the actions). */
export const MANAGE_VIEWS: { to: string; label: string }[] = [
  { to: '/agents', label: 'Agents' },
  { to: '/teams', label: 'Teams' },
  { to: '/models', label: 'Models' },
  { to: '/mcp', label: 'MCP' },
  { to: '/templates', label: 'Templates' },
  { to: '/studio', label: 'Agent Studio' },
  { to: '/observability', label: 'Observability' },
  { to: '/apps', label: 'Apps' },
]

export interface NavSection {
  /** Stable identity consumers branch on — never the display string, which
   *  may not exist at all. */
  id: string
  /** Optional display header. A section without one renders as a bare list:
   *  the Work views need no label above them, the views are the label. */
  title?: string
  items: NavItem[]
  /** The whole section is role-gated (hidden + route-bounced for members). */
  adminOnly?: boolean
}

export const NAV: NavSection[] = [
  {
    id: 'work',
    items: [
      // `/home`, not `/`. The rail entry names the CONTAINER, so every tab
      // inside it (`/home/inbox`, `/home/boards`, `/home/fleet`) lights this
      // one item — which is what the nesting rule is for. Pointing it at
      // `/home/inbox` would be precise and would leave every other Home tab
      // lighting nothing.
      { to: '/home', label: 'Inbox', icon: Inbox },
      { to: '/comms', label: 'Comms', icon: MessageCircle, badge: 'comms' },
      { to: '/plan', label: 'Plan', icon: CalendarRange, badge: 'plan' },
      { to: '/boards', label: 'Boards', icon: LayoutGrid },
      { to: '/research', label: 'Research', icon: Telescope, badge: 'research' },
      { to: '/knowledge', label: 'Knowledge', icon: BookOpen },
      { to: '/artifacts', label: 'Files', icon: FileBox },
    ],
  },
  {
    id: 'manage',
    // Not a blanket admin section anymore: members see whichever Manage views
    // they've been granted (deniedViews computes the default-denied set).
    title: 'Manage',
    items: [
      { to: '/agents', label: 'Agents', icon: Bot },
      { to: '/teams', label: 'Teams', icon: Users },
      { to: '/models', label: 'Models', icon: Cpu },
      { to: '/mcp', label: 'MCP', icon: PlugZap },
      { to: '/templates', label: 'Templates', icon: LayoutTemplate },
      { to: '/studio', label: 'Agent Studio', icon: Settings2 },
      { to: '/observability', label: 'Observability', icon: Activity },
      { to: '/apps', label: 'Apps', icon: Hexagon },
    ],
  },
  // Settings and Admin live in the USER MENU (top strip), not the rail: they
  // are about the person and the instance, not the work. Settings stays
  // always-reachable (never gateable); /admin stays role-locked via the
  // explicit ADMIN_VIEWS entry below.
]

/** Routes members can never reach regardless of grants. Manage views moved to
 *  the grantable set; Admin (in the user menu) stays role-locked via the
 *  explicit '/admin' entry — it must never depend on a nav section existing. */
export const ADMIN_VIEWS: string[] = [
  ...new Set([
    ...NAV.flatMap((s) => s.items.filter((i) => s.adminOnly || i.adminOnly).map((i) => i.to)),
    '/admin',
  ]),
]

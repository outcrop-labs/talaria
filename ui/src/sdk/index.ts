// @talaria/sdk — the surface Talaria apps build against.
//
// A Talaria app is a self-contained codebase in apps/<slug>/ that compiles
// INTO the deployment and renders as native platform UI. It ships:
//   talaria.json  manifest (name, icon, surfaces) — read by host + admin UI
//   app.tsx       defineApp({ work?, manage?, settings? }) — React surfaces
//   server.ts     optional defineAppServer(...) — API under /api/apps/<slug>/*
//
// Apps import ONLY from '@talaria/sdk' (+ react): the SDK re-exports the
// Mercury UI kit, session/query hooks, and a fetch helper wired to the app's
// own server routes. Everything runs under the signed-in user's session, so
// every platform permission and ACL applies unchanged — an app can never do
// more than the person using it.
import { useQuery, useQueryClient } from '@tanstack/react-query'
import type { ComponentType } from 'react'

// ── App definition ─────────────────────────────────────────────────────────
export interface AppSurfaces {
  /** The app's main view, shown in the Work section of the nav. */
  work?: ComponentType
  /** Admin-ish controls, shown in the Manage section (default-denied for members, grantable). */
  manage?: ComponentType
  /** A panel added to each user's Settings page. */
  settings?: ComponentType
}

/** Identity: apps/<slug>/app.tsx default-exports this. */
export const defineApp = (surfaces: AppSurfaces): AppSurfaces => surfaces

// ── UI kit (Mercury design system) ─────────────────────────────────────────
export { Button, buttonClasses, type ButtonProps } from '@/components/ui/button'
export { IconButton } from '@/components/ui/icon-button'
export { Input, type InputProps } from '@/components/ui/input'
export { Textarea, type TextareaProps } from '@/components/ui/textarea'
export { Select, type SelectProps } from '@/components/ui/select'
export { Combobox, type ComboOption } from '@/components/ui/combobox'
export { Modal } from '@/components/ui/modal'
export { Panel, type PanelProps } from '@/components/ui/panel'
export { Chip, StatusDot, DangerLink, type ChipProps, type ChipTone, type DotStatus } from '@/components/ui/chip'
export { EmptyState } from '@/components/ui/empty-state'
export { Skeleton, SkeletonRows, SkeletonCard } from '@/components/ui/skeleton'
export { InfoTip } from '@/components/ui/info-tip'
export { Avatar } from '@/components/ui/avatar'
export { Markdown } from '@/components/ui/markdown'
export { confirm, alert, prompt } from '@/components/ui/confirm'
export { useContextMenu, DropdownMenu, type ContextMenuItem, type ContextMenuEntry } from '@/components/ui/context-menu'
export { Tabs, type TabItem } from '@/components/ui/tabs'
export { Checkbox, Radio, Toggle } from '@/components/ui/checkbox'
export { SectionHeader } from '@/components/ui/section-header'
export { Segmented, type SegmentedOption } from '@/components/ui/segmented'
export { SaveButton, useSavedFlash } from '@/components/ui/save-button'
export { CopyButton, CopyLinkButton } from '@/components/ui/copy-link-button'
export { CloseButton } from '@/components/ui/close-button'
export { StatCard } from '@/components/ui/stat-card'
export { Disclosure } from '@/components/ui/disclosure'
export { CodeBlock } from '@/components/ui/code-block'
export { Steps } from '@/components/ui/steps'
export { InlineCreate } from '@/components/ui/inline-create'
export { Kbd, KeyHint } from '@/components/ui/kbd'
export { Generating, GeneratingDots, GeneratingOverlay } from '@/components/ui/generating'
export { RichEditor, type RichEditorHandle } from '@/components/ui/rich-editor'
export { controlSizes, submitOnEnter, inlineEditKeys, type ControlSize } from '@/components/ui/control'
export { cn } from '@/lib/cn'

// ── Session + data hooks ───────────────────────────────────────────────────
export { useSession as useMe, useHasPerm, useIsAdmin, type SessionUser } from '@/lib/session'
export { useQuery, useMutation, useQueryClient, keepPreviousData } from '@tanstack/react-query'

/** JSON fetch against any platform API (runs as the signed-in user). */
export async function api<T = unknown>(path: string, init?: RequestInit & { json?: unknown }): Promise<T> {
  const { json: body, ...rest } = init ?? {}
  const res = await fetch(path, {
    credentials: 'same-origin',
    ...rest,
    ...(body !== undefined
      ? { headers: { 'content-type': 'application/json', ...(rest.headers ?? {}) }, body: JSON.stringify(body) }
      : {}),
  })
  if (!res.ok) {
    const j = (await res.json().catch(() => ({}))) as { error?: string }
    throw new Error(j.error ?? `${res.status}`)
  }
  return res.json() as Promise<T>
}

/** Fetch helpers bound to THIS app's server routes (/api/apps/<slug>/…). */
export function appApi(slug: string) {
  const base = `/api/apps/${slug}`
  return {
    get: <T = unknown>(path: string) => api<T>(`${base}/${path}`),
    post: <T = unknown>(path: string, body?: unknown) => api<T>(`${base}/${path}`, { method: 'POST', json: body ?? {} }),
    put: <T = unknown>(path: string, body?: unknown) => api<T>(`${base}/${path}`, { method: 'PUT', json: body ?? {} }),
    patch: <T = unknown>(path: string, body?: unknown) => api<T>(`${base}/${path}`, { method: 'PATCH', json: body ?? {} }),
    del: <T = unknown>(path: string, body?: unknown) =>
      api<T>(`${base}/${path}`, { method: 'DELETE', ...(body !== undefined ? { json: body } : {}) }),
  }
}

/** React-query wrapper over an app server GET — the common read path. */
export function useAppQuery<T = unknown>(slug: string, path: string) {
  return useQuery({
    queryKey: ['app', slug, path],
    queryFn: () => appApi(slug).get<T>(path),
  })
}

/** Invalidate this app's queries after a write. */
export function useAppInvalidate(slug: string) {
  const qc = useQueryClient()
  return (path?: string) =>
    qc.invalidateQueries({ queryKey: path ? ['app', slug, path] : ['app', slug] })
}

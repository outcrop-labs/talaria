// @talaria/sdk — the surface Talaria apps build against.
//
// A Talaria app is a self-contained codebase in apps/<slug>/ that compiles
// INTO the deployment and renders as native platform UI. It ships:
//   talaria.json  manifest (name, icon, surfaces) — read by host + admin UI
//   app.ts        defineApp({ work?, manage?, settings? }) — Svelte surfaces
//   server.ts     optional defineAppServer(...) — API under /api/apps/<slug>/*
//
// Apps import ONLY from '@talaria/sdk' (+ svelte): the SDK re-exports the
// Mercury UI kit, session/query hooks, and a fetch helper wired to the app's
// own server routes. Everything runs under the signed-in user's session, so
// every platform permission and ACL applies unchanged — an app can never do
// more than the person using it.
import { createQuery, useQueryClient } from '@tanstack/svelte-query'
import type { Component, ComponentProps } from 'svelte'

// ── App definition ─────────────────────────────────────────────────────────
export interface AppSurfaces {
  /** The app's main view, shown in the Work section of the nav. */
  work?: Component
  /** Admin-ish controls, shown in the Manage section (default-denied for members, grantable). */
  manage?: Component
  /** A panel added to each user's Settings page. */
  settings?: Component
}

/** Identity: apps/<slug>/app.ts default-exports this. */
export const defineApp = (surfaces: AppSurfaces): AppSurfaces => surfaces

// ── UI kit (Mercury design system) ─────────────────────────────────────────
// Prop types that used to live beside the React components now derive from the
// Svelte components themselves — the old type-alias names stay exported so
// app code keeps compiling.
import Button from '@/components/ui/Button.svelte'
import Input from '@/components/ui/Input.svelte'
import Textarea from '@/components/ui/Textarea.svelte'
import Select from '@/components/ui/Select.svelte'
export { Button, Input, Textarea, Select }
export type ButtonProps = ComponentProps<typeof Button>
export type InputProps = ComponentProps<typeof Input>
export type TextareaProps = ComponentProps<typeof Textarea>
export type SelectProps = ComponentProps<typeof Select>
export { buttonClasses } from '@/components/ui/button'
// Motion grammar (ANIMATIONS.md): apps animate with the same reduced-motion-
// aware wrappers and presets as the host, via the SDK — never a relative
// reach into ui/src.
export { fade, fly, scale, slide, flip, QUICK, POP, PANEL, LIST } from '@/lib/motion'
export { default as IconButton } from '@/components/ui/IconButton.svelte'
export { default as ViewHeader } from '@/components/ui/ViewHeader.svelte'
export { default as Combobox } from '@/components/ui/Combobox.svelte'
export { type ComboOption } from '@/components/ui/combobox'
export { default as Modal } from '@/components/ui/Modal.svelte'
export { default as Panel, type PanelProps } from '@/components/ui/Panel.svelte'
export { default as Chip } from '@/components/ui/Chip.svelte'
export { default as StatusDot } from '@/components/ui/StatusDot.svelte'
export { default as DangerLink } from '@/components/ui/DangerLink.svelte'
export { type ChipProps, type ChipTone, type DotStatus } from '@/components/ui/chip'
export { default as EmptyState } from '@/components/ui/EmptyState.svelte'
export { default as Skeleton } from '@/components/ui/Skeleton.svelte'
export { default as SkeletonRows } from '@/components/ui/SkeletonRows.svelte'
export { default as SkeletonCard } from '@/components/ui/SkeletonCard.svelte'
export { default as InfoTip } from '@/components/ui/InfoTip.svelte'
export { default as Avatar } from '@/components/ui/Avatar.svelte'
export { default as Markdown } from '@/components/ui/Markdown.svelte'
export { confirm, alert, prompt } from '@/components/ui/confirm.svelte'
export { useContextMenu, type ContextMenuItem, type ContextMenuEntry } from '@/components/ui/context-menu.svelte'
export { default as DropdownMenu } from '@/components/ui/DropdownMenu.svelte'
export { default as Tabs } from '@/components/ui/Tabs.svelte'
export { type TabItem } from '@/components/ui/tabs'
export { default as Checkbox } from '@/components/ui/Checkbox.svelte'
export { default as Radio } from '@/components/ui/Radio.svelte'
export { default as Toggle } from '@/components/ui/Toggle.svelte'
export { default as SectionHeader } from '@/components/ui/SectionHeader.svelte'
export { default as Segmented } from '@/components/ui/Segmented.svelte'
export { type SegmentedOption } from '@/components/ui/segmented'
export { default as SaveButton } from '@/components/ui/SaveButton.svelte'
export { useSavedFlash } from '@/components/ui/save-button.svelte'
export { default as CopyButton } from '@/components/ui/CopyButton.svelte'
export { default as CopyLinkButton } from '@/components/ui/CopyLinkButton.svelte'
export { default as CloseButton } from '@/components/ui/CloseButton.svelte'
export { default as StatCard } from '@/components/ui/StatCard.svelte'
export { default as Disclosure } from '@/components/ui/Disclosure.svelte'
export { default as CodeBlock } from '@/components/ui/CodeBlock.svelte'
export { default as Steps } from '@/components/ui/Steps.svelte'
export { default as InlineCreate } from '@/components/ui/InlineCreate.svelte'
export { default as Kbd } from '@/components/ui/Kbd.svelte'
export { default as Generating } from '@/components/ui/Generating.svelte'
export { default as GeneratingDots } from '@/components/ui/GeneratingDots.svelte'
export { default as GeneratingOverlay } from '@/components/ui/GeneratingOverlay.svelte'
// The waiting marks: "an agent is working right now", as opposed to Generating
// ("model output is being written") or Skeleton ("a fetch hasn't resolved").
// An app can't add rows to the host's site table, so it names its own site
// inline — `site={{ key: 'my-app/summarise', role: 'reasoning' }}` — and gets a
// pick hashed off the host session's seed, re-rolling with the rest of the
// cockpit. `role` is required: an undeclared wait can't be paced.
export { default as Waiting } from '@/components/ui/Waiting.svelte'
export { default as WaitingMark } from '@/components/ui/WaitingMark.svelte'
export type { InlineWaitingSite } from '@/lib/waiting/rotation'
export type { WaitingRole, WaitingSlot } from '@/lib/waiting/registry'
export { default as RichEditor } from '@/components/ui/RichEditor.svelte'
export { type RichEditorHandle } from '@/components/ui/rich-editor'
export { controlSizes, submitOnEnter, inlineEditKeys, type ControlSize } from '@/components/ui/control'
export { cn } from '@/lib/cn'

// ── Session + data hooks ───────────────────────────────────────────────────
export { useSession as useMe, useHasPerm, useIsAdmin, type SessionUser } from '@/lib/session'
export { createQuery, createMutation, useQueryClient, keepPreviousData } from '@tanstack/svelte-query'

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

/** Svelte-query wrapper over an app server GET — the common read path.
 *  `path` may be a getter (`() => string`) so a path built from reactive
 *  state re-keys the query as that state changes (options are a function,
 *  re-run on rune changes). Call during component init. */
export function useAppQuery<T = unknown>(slug: string, path: string | (() => string)) {
  return createQuery(() => {
    const p = typeof path === 'function' ? path() : path
    return {
      queryKey: ['app', slug, p],
      queryFn: () => appApi(slug).get<T>(p),
    }
  })
}

/** Invalidate this app's queries after a write. Call during component init
 *  (it grabs the query client from context); the returned function is safe
 *  anywhere. */
export function useAppInvalidate(slug: string) {
  const qc = useQueryClient()
  return (path?: string) =>
    qc.invalidateQueries({ queryKey: path ? ['app', slug, path] : ['app', slug] })
}

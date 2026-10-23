// @talaria/sdk — the surface Talaria apps build against.
//
// A Talaria app is a self-contained TypeScript codebase in apps/<slug>/ that
// this instance compiles and runs without a host rebuild. It ships:
//   talaria.json  manifest (name, icon, surfaces) — read by host + admin UI
//   app.ts        defineApp({ work?, manage?, settings? }) — Svelte surfaces
//   server.ts     optional defineAppServer(...) — API under /api/apps/<slug>/*
//
// Apps import ONLY from '@talaria/sdk' (+ svelte): the SDK re-exports the
// Mercury UI kit, session/query hooks, and a fetch helper wired to the app's
// own server routes. Everything runs under the signed-in user's session, so
// every platform permission and ACL applies unchanged — an app can never do
// more than the person using it. App authors never write Rust.

import type { Component, ComponentProps } from 'svelte'

// ── App definition ─────────────────────────────────────────────────────────
export interface AppSurfaces {
  /** The app's main view at /x/<slug>, and every route below it
   *  (/x/<slug>/items/42): a tile in the top dock's Apps group. */
  work?: Component
  /** Admin-ish controls at /x/<slug>/manage and the routes below it, listed
   *  in the manage area behind the dock's gear (default-denied for members,
   *  grantable). */
  manage?: Component
  /** A tab added to the Settings page of people with the manage grant; one
   *  page, with no routes below it. */
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
export { default as Popover } from '@/components/ui/Popover.svelte'
export { default as Tabs } from '@/components/ui/Tabs.svelte'
export { type TabItem } from '@/components/ui/tabs'
export { default as Checkbox } from '@/components/ui/Checkbox.svelte'
export { default as Radio } from '@/components/ui/Radio.svelte'
export { default as Toggle } from '@/components/ui/Toggle.svelte'
export { default as SectionHeader } from '@/components/ui/SectionHeader.svelte'
export { default as Segmented } from '@/components/ui/Segmented.svelte'
export { type SegmentedOption } from '@/components/ui/segmented'
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

// ── Query state: loading, broken, or resolved ─────────────────────────────
// The host rule, handed to apps: a failed read renders as a failure, never as
// "there are none of these". `QueryState` takes the whole query and your own
// skeleton and gives you the error branch for free; `listQuery` does the same
// for a list you lay out yourself. See docs/sdk/client.md.
export { default as QueryState } from '@/components/ui/QueryState.svelte'
export { default as QueryError } from '@/components/ui/QueryError.svelte'
export { listQuery, type ListQuery, type QueryLike, type QueryErrorProps } from '@/components/ui/query-state'

// Host primitives an app's records and registries are built from — the same
// components the core surfaces use, so an app's list reads like the host's.
export { default as SortHeader } from '@/components/ui/SortHeader.svelte'
export { default as AddRow } from '@/components/ui/AddRow.svelte'
export { default as RenameField } from '@/components/ui/RenameField.svelte'
export { default as RecordTitle } from '@/components/ui/RecordTitle.svelte'
export { default as LibraryPane, type LibraryGroup } from '@/components/ui/LibraryPane.svelte'

// ── Session + data hooks ───────────────────────────────────────────────────
export { useSession as useMe, useHasPerm, useIsAdmin, type SessionUser } from '@/lib/session'
export { createQuery, createMutation, useQueryClient, keepPreviousData } from '@tanstack/svelte-query'
// The live firehose: run, notification, brief and message transitions, ids
// only. One shared connection per tab — subscribe through this, never a second
// EventSource of your own.
export { useUserEvents, type UserEvent } from '@/lib/user-events.svelte'

// ── HTTP: the host's own door ──────────────────────────────────────────────
// `api()` and `appApi()` go through the same verbs the host's surfaces use
// (ui/src/lib/fetch-json.ts): a non-2xx rejects with an `HttpError` carrying
// the status and the server's own sentence, a 2xx that isn't JSON rejects
// with a sentence that says so, and every read carries a deadline.
export { HttpError, errorMessage, getStream, postStream } from '@/lib/fetch-json'
export { openStream } from '@/lib/sse'
export { api, appApi, appQueryKey, type ApiInit, type AppApi, type AppApiInit } from '@/lib/app-client'

// ── The app this surface belongs to ────────────────────────────────────────
// `useApp()` says which app; the hooks default to it, reading the slug once at
// init (ui/src/lib/app-hooks.ts says why).
export type { AppContext, AppSurfaceName } from '@/lib/app-context'
export type { AppManifest } from '@/lib/apps'
export { useApp, useAppApi, useAppQuery, useAppInvalidate, type AppQueryOptions } from '@/lib/app-hooks'

// ── Routes inside the app ──────────────────────────────────────────────────
// An app owns every route below its surfaces: /x/<slug>/<rest> renders the
// work surface and /x/<slug>/manage/<rest> the manage surface, so a list, its
// record pages and its tabs are real addresses — linkable, bookmarkable, and
// stepped through by Back and Forward. `useAppRoute()` says where the surface
// is; `appHref` and `appNavigate` go elsewhere in the app, relative to the
// surface. A settings tab is one page, with no routes below it.
export {
  useAppRoute,
  appHref,
  appNavigate,
  type AppRoute,
  type AppHrefOptions,
  type AppNavigateOptions,
  type RouteParams,
} from '@/lib/app-route'

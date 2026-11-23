import { Settings } from '@lucide/svelte'
import type { MenuIcon } from './context-menu.svelte'

// THE settings glyph. Every door into a settings surface draws its gear from
// this file: the icon-only trigger (`SettingsButton`), a menu entry that opens
// settings (`cogMenuIcon`), and a gear beside a visible label (`SettingsGlyph`).
//
// Why lucide `Settings`, the standard gear: `Settings2` and `SlidersHorizontal`
// read as adjust or filter, and `Cog` is the dock's Manage tile (app chrome, a
// destination). The default stroke (2) stays: at 14px a 1.5 stroke thins the
// teeth to under a pixel. `scripts/settings-glyph.mjs` fails a gear imported
// anywhere else, so per-view copies cannot come back.

/** The one gear. Use it only beside a visible label; an icon-only door is a
 *  `SettingsButton`. */
export const SettingsGlyph = Settings

export type CogSize = 'sm' | 'md'

/** Glyph px per tile size, always half the tile: IconButton sm 28 → 14,
 *  md 32 → 16. A gear beside a visible label (a menu entry, a link) is `sm`. */
export const COG: Record<CogSize, number> = { sm: 14, md: 16 }

/** A menu entry that opens a settings surface wears this. */
export const cogMenuIcon: MenuIcon = [Settings, { size: COG.sm }]

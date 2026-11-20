// The settings door's structure, server-rendered: the light CI ui job runs
// this on every pull request, while the browser proof (names, hit areas and
// contrast in both themes, ui/e2e/settings-cog.spec.ts) runs in full mode only.
import { render } from 'svelte/server'
import { describe, expect, it } from 'vitest'
import SettingsButton from './SettingsButton.svelte'
import { COG, SettingsGlyph, cogMenuIcon } from './settings-cog'

const html = (props: Record<string, unknown>) => render(SettingsButton, { props: props as never }).body

/** The opening `<button …>` tag of a rendered door. */
const buttonTag = (body: string) => body.match(/<button\b[^>]*>/)?.[0] ?? ''

describe('settings-cog', () => {
  it('is one gear: lucide Settings, on the door, the menu entry and beside a label', () => {
    // No lucide import here: scripts/settings-glyph.mjs allows the gear in
    // settings-cog.ts alone, tests included.
    expect(render(SettingsGlyph, { props: {} }).body).toMatch(/class="[^"]*\blucide-settings\b(?!-)/)
    expect(cogMenuIcon[0]).toBe(SettingsGlyph)
    expect(cogMenuIcon[1]).toEqual({ size: COG.sm })
  })

  it('keeps the glyph at half the tile', () => {
    expect(COG).toEqual({ sm: 14, md: 16 })
  })
})

describe('SettingsButton', () => {
  it('names its scope in the tooltip and the accessible name, the same words', () => {
    const tag = buttonTag(html({ label: 'Board settings' }))
    expect(tag).toContain('title="Board settings"')
    expect(tag).toContain('aria-label="Board settings"')
    expect(tag).toContain('type="button"')
  })

  it('draws exactly one standard gear, never the sliders', () => {
    const body = html({ label: 'Column settings' })
    expect(body.match(/lucide-settings\b/g)).toHaveLength(1)
    expect(body).not.toContain('lucide-settings-2')
    expect(body).not.toContain('lucide-sliders')
  })

  it('sizes the target and the glyph from one scale: sm 28/14 by default, md 32/16', () => {
    const sm = html({ label: 'Column settings' })
    expect(buttonTag(sm)).toMatch(/\bh-7 w-7\b/)
    expect(sm).toMatch(/<svg[^>]*\bwidth="14"[^>]*\bheight="14"/)
    const md = html({ label: 'Board settings', size: 'md' })
    expect(buttonTag(md)).toMatch(/\bh-8 w-8\b/)
    expect(md).toMatch(/<svg[^>]*\bwidth="16"[^>]*\bheight="16"/)
  })

  it('brightens on focus as well as hover, and marks the open state', () => {
    const rest = buttonTag(html({ label: 'View settings' }))
    expect(rest).toContain('text-muted')
    expect(rest).toContain('hover:text-fg')
    expect(rest).toContain('focus:text-fg')
    const open = buttonTag(html({ label: 'View settings', active: true, 'aria-expanded': true }))
    expect(open).toContain('bg-raised text-fg')
    expect(open).toContain('aria-expanded="true"')
  })

  it('passes the dialog and test hooks through, and wears the door marker', () => {
    const tag = buttonTag(html({ label: 'Channel settings', 'aria-haspopup': 'dialog' }))
    expect(tag).toContain('aria-haspopup="dialog"')
    expect(tag).toContain('data-settings-door')
  })
})

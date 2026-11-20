// The settings door's structure, server-rendered: the light CI ui job runs
// this on every pull request, while the browser proof (names, hit areas and
// contrast in both themes, ui/e2e/settings-cog.spec.ts) runs in full mode only.
import type { ComponentProps } from 'svelte'
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
    // `MenuIcon` is a snippet OR a [component, props] pair: compared whole,
    // not indexed, so the snippet branch never needs narrowing.
    expect(cogMenuIcon).toEqual([SettingsGlyph, { size: COG.sm }])
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

  it('refuses, by type, a bare "Settings", a class and a second name', () => {
    // These lines are for svelte-check (CI's ui typecheck), which reads this
    // file: each `@ts-expect-error` fails the job if its line ever compiles,
    // so widening `label` to `string` or letting `class` or `aria-label`
    // back into the props turns the job red. Vitest only sees the objects.
    type DoorProps = ComponentProps<typeof SettingsButton>
    // @ts-expect-error a bare "Settings" names no scope
    void ({ label: 'Settings' } satisfies DoorProps)
    // @ts-expect-error no class: the tile and the glyph come from one scale
    void ({ label: 'Board settings', class: 'h-4' } satisfies DoorProps)
    // @ts-expect-error the label is the accessible name: nothing renames it
    void ({ label: 'Board settings', 'aria-label': 'Configure columns' } satisfies DoorProps)
    // @ts-expect-error nor points the name at other words
    void ({ label: 'Board settings', 'aria-labelledby': 'columns-heading' } satisfies DoorProps)
    // A scoped label is all a door needs.
    expect(({ label: 'Board settings' } satisfies DoorProps).label).toBe('Board settings')
  })

  it('passes the dialog and test hooks through, and wears the door marker', () => {
    const tag = buttonTag(html({ label: 'Channel settings', 'aria-haspopup': 'dialog' }))
    expect(tag).toContain('aria-haspopup="dialog"')
    expect(tag).toContain('data-settings-door')
  })
})

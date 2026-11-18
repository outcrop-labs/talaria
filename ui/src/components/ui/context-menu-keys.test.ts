import assert from 'node:assert/strict'
import { test } from 'vitest'
import { isMenuKey, menuKeyStep, placeMenu, submenuSide, type MenuKeyContext } from './context-menu-keys'

const ctx = (over: Partial<MenuKeyContext> = {}): MenuKeyContext => ({
  active: -1,
  labels: ['Open', 'Copy link', 'Pin', 'Notifications', 'Delete…'],
  hasChildren: (i) => i === 3,
  inSub: false,
  ...over,
})

test('the Menu key and Shift+F10 open a menu; F10 alone and other keys do not', () => {
  assert.equal(isMenuKey({ key: 'ContextMenu', shiftKey: false }), true)
  assert.equal(isMenuKey({ key: 'F10', shiftKey: true }), true)
  assert.equal(isMenuKey({ key: 'F10', shiftKey: false }), false)
  assert.equal(isMenuKey({ key: 'Enter', shiftKey: true }), false)
})

test('arrows move and wrap; from nothing they land on the first or the last', () => {
  assert.deepEqual(menuKeyStep('ArrowDown', ctx()), { kind: 'move', active: 0 })
  assert.deepEqual(menuKeyStep('ArrowUp', ctx()), { kind: 'move', active: 4 })
  assert.deepEqual(menuKeyStep('ArrowDown', ctx({ active: 1 })), { kind: 'move', active: 2 })
  assert.deepEqual(menuKeyStep('ArrowDown', ctx({ active: 4 })), { kind: 'move', active: 0 })
  assert.deepEqual(menuKeyStep('ArrowUp', ctx({ active: 0 })), { kind: 'move', active: 4 })
  assert.deepEqual(menuKeyStep('Home', ctx({ active: 3 })), { kind: 'move', active: 0 })
  assert.deepEqual(menuKeyStep('End', ctx({ active: 0 })), { kind: 'move', active: 4 })
  // An empty menu has nowhere to go.
  assert.deepEqual(menuKeyStep('ArrowDown', ctx({ labels: [] })), { kind: 'none' })
})

test('Enter and Space activate, or open a submenu on its parent', () => {
  assert.deepEqual(menuKeyStep('Enter', ctx({ active: 2 })), { kind: 'activate', index: 2 })
  assert.deepEqual(menuKeyStep(' ', ctx({ active: 0 })), { kind: 'activate', index: 0 })
  assert.deepEqual(menuKeyStep('Enter', ctx({ active: 3 })), { kind: 'open-sub', index: 3 })
  // Nothing active (a pointer open): Enter does nothing rather than guess.
  assert.deepEqual(menuKeyStep('Enter', ctx()), { kind: 'none' })
})

test('Right opens a submenu, Left and Escape come back out of one', () => {
  assert.deepEqual(menuKeyStep('ArrowRight', ctx({ active: 3 })), { kind: 'open-sub', index: 3 })
  assert.deepEqual(menuKeyStep('ArrowRight', ctx({ active: 2 })), { kind: 'none' })
  assert.deepEqual(menuKeyStep('ArrowLeft', ctx({ active: 1, inSub: true })), { kind: 'close-sub' })
  assert.deepEqual(menuKeyStep('Escape', ctx({ active: 1, inSub: true })), { kind: 'close-sub' })
  assert.deepEqual(menuKeyStep('ArrowLeft', ctx({ active: 1 })), { kind: 'none' })
})

test('Escape and Tab close the menu from the top level', () => {
  assert.deepEqual(menuKeyStep('Escape', ctx({ active: 2 })), { kind: 'close' })
  assert.deepEqual(menuKeyStep('Tab', ctx({ active: 2 })), { kind: 'close' })
  assert.deepEqual(menuKeyStep('Tab', ctx({ active: 0, inSub: true })), { kind: 'close' })
})

test('a letter jumps to the next item that starts with it, wrapping', () => {
  assert.deepEqual(menuKeyStep('c', ctx()), { kind: 'move', active: 1 })
  assert.deepEqual(menuKeyStep('D', ctx({ active: 1 })), { kind: 'move', active: 4 })
  // From Open, "o" finds no other O and comes back round to Open itself.
  assert.deepEqual(menuKeyStep('o', ctx({ active: 0 })), { kind: 'move', active: 0 })
  assert.deepEqual(menuKeyStep('z', ctx()), { kind: 'none' })
  assert.deepEqual(menuKeyStep('Shift', ctx()), { kind: 'none' })
})

const vp = { width: 400, height: 600 }

test('a pointer open puts the corner at the pointer, and flips off the edges', () => {
  const size = { width: 180, height: 200 }
  assert.deepEqual(placeMenu(size, vp, { x: 50, y: 60 }), { left: 50, top: 60 })
  // Near the right edge: it opens to the left of the pointer.
  assert.deepEqual(placeMenu(size, vp, { x: 350, y: 60 }), { left: 170, top: 60 })
  // Near the bottom: it opens upward.
  assert.deepEqual(placeMenu(size, vp, { x: 50, y: 550 }), { left: 50, top: 350 })
  // A menu taller than the screen pins to the top margin (it scrolls inside).
  assert.deepEqual(placeMenu({ width: 180, height: 900 }, vp, { x: 50, y: 300 }), { left: 50, top: 8 })
})

test('an anchored open sits under its element, or above it when the room below is short', () => {
  const size = { width: 180, height: 200 }
  const anchor = { top: 100, bottom: 124, left: 20, right: 44 }
  assert.deepEqual(placeMenu(size, vp, { x: 20, y: 124, anchor }), { left: 20, top: 128 })
  const low = { top: 500, bottom: 524, left: 300, right: 324 }
  // No room below: above it. Too far right: clamped inside the margin.
  assert.deepEqual(placeMenu(size, vp, { x: 300, y: 524, anchor: low }), { left: 212, top: 296 })
})

test('a submenu opens on the side with room', () => {
  assert.equal(submenuSide({ left: 20, right: 200 }, 160, 400), 'right')
  assert.equal(submenuSide({ left: 220, right: 380 }, 160, 400), 'left')
  // Neither side fits a phone-width screen: the roomier one wins.
  assert.equal(submenuSide({ left: 10, right: 300 }, 200, 320), 'right')
  assert.equal(submenuSide({ left: 200, right: 300 }, 260, 320), 'left')
})

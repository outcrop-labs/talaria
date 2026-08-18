import assert from 'node:assert/strict'
import { test } from 'vitest'
import { splitLayoutClasses } from '@/components/ui/button'

// A bloomed button gains a wrapper span, and the wrapper becomes the flex
// child. Anything that positions the button inside its PARENT has to move
// there with it, or it stays valid, stays applied, and silently does nothing.

test('nothing in, nothing out', () => {
  assert.deepEqual(splitLayoutClasses(), { outer: '', inner: '' })
  assert.deepEqual(splitLayoutClasses(null), { outer: '', inner: '' })
  assert.deepEqual(splitLayoutClasses(''), { outer: '', inner: '' })
})

test('margins and self-alignment go OUT to the wrapper', () => {
  // The real regression this exists for.
  assert.deepEqual(splitLayoutClasses('ml-auto shrink-0'), { outer: 'ml-auto shrink-0', inner: '' })
  assert.deepEqual(splitLayoutClasses('self-start'), { outer: 'self-start', inner: '' })
  assert.deepEqual(splitLayoutClasses('-mt-1'), { outer: '-mt-1', inner: '' })
  assert.deepEqual(splitLayoutClasses('order-2 basis-40'), { outer: 'order-2 basis-40', inner: '' })
})

test('flex-item sizing and full width go out', () => {
  assert.deepEqual(splitLayoutClasses('flex-1'), { outer: 'flex-1', inner: '' })
  assert.deepEqual(splitLayoutClasses('w-full'), { outer: 'w-full', inner: '' })
  assert.deepEqual(splitLayoutClasses('grow'), { outer: 'grow', inner: '' })
})

test('the control\'s OWN styling stays on the control', () => {
  // Padding especially: moved to the wrapper it would grow the wrapper and
  // push the button off its baseline rather than pad the label.
  const { outer, inner } = splitLayoutClasses('px-6 text-danger font-bold rounded-none')
  assert.equal(outer, '')
  assert.equal(inner, 'px-6 text-danger font-bold rounded-none')
})

test('a mix is split, and order within each side is preserved', () => {
  assert.deepEqual(splitLayoutClasses('ml-auto px-6 w-full text-xs'), {
    outer: 'ml-auto w-full',
    inner: 'px-6 text-xs',
  })
})

test('variant prefixes are seen through', () => {
  // `sm:ml-auto` is a margin; matching the raw string would miss it.
  assert.deepEqual(splitLayoutClasses('sm:ml-auto hover:text-fg'), {
    outer: 'sm:ml-auto',
    inner: 'hover:text-fg',
  })
})

test('lookalikes that are NOT layout stay on the control', () => {
  // `min-w-` and `max-w-` size the control itself; `w-` alone is the one that
  // means "fill the parent". `mono`/`muted` merely start with m.
  const { outer, inner } = splitLayoutClasses('min-w-0 max-w-xs font-mono text-muted')
  assert.equal(outer, '')
  assert.equal(inner, 'min-w-0 max-w-xs font-mono text-muted')
})

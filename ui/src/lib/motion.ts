// Motion grammar for the Svelte side, replacing framer-motion's
// <MotionConfig reducedMotion="user">: every transition in the app goes
// through these wrappers, which collapse to a fade-only (entrances lose
// travel/scale) when the OS asks for reduced motion — spec §9. CSS motifs are
// handled by the prefers-reduced-motion block in styles.css.
//
// Usage is identical to svelte/transition:
//   <div transition:fade>            import { fade } from '@/lib/motion'
//   <div in:fly={{ y: 8 }}>          import { fly } from '@/lib/motion'
import {
  fade as svFade,
  fly as svFly,
  scale as svScale,
  slide as svSlide,
  type FadeParams,
  type FlyParams,
  type ScaleParams,
  type SlideParams,
  type TransitionConfig,
} from 'svelte/transition'

function reducedMotion(): boolean {
  return typeof window !== 'undefined' && window.matchMedia('(prefers-reduced-motion: reduce)').matches
}

/** Reduced motion degrades to a quick fade rather than nothing: an element
 *  that pops in with zero signal reads as a glitch, not as calm. */
function degrade(node: Element, params?: { duration?: number; delay?: number }): TransitionConfig {
  return svFade(node, { duration: Math.min(params?.duration ?? 150, 150), delay: params?.delay })
}

export function fade(node: Element, params?: FadeParams): TransitionConfig {
  return svFade(node, params)
}

export function fly(node: Element, params?: FlyParams): TransitionConfig {
  return reducedMotion() ? degrade(node, params) : svFly(node, params)
}

export function scale(node: Element, params?: ScaleParams): TransitionConfig {
  return reducedMotion() ? degrade(node, params) : svScale(node, params)
}

export function slide(node: Element, params?: SlideParams): TransitionConfig {
  return reducedMotion() ? degrade(node, params) : svSlide(node, params)
}

// House defaults, so surfaces feel like one system rather than 141 opinions:
//   modals/popovers  in:scale={POP}  out:fade={QUICK}
//   panels/drawers   fly with a small travel (8–16px), 150–200ms
//   list/row reveal  slide (height) or fade, ≤150ms
//   banners          slide
export const QUICK: FadeParams = { duration: 120 }
export const POP: ScaleParams = { duration: 160, start: 0.96 }
export const PANEL: FlyParams = { duration: 180, y: 8 }

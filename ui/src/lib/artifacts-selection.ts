// WHERE YOU WERE STANDING IN FILES, remembered by Artifacts itself.
//
// See `view-memory.ts` for why this is a stored SELECTION rather than a stored
// href, and for the two rules the view has to follow when using it.
//
// Artifacts' location has three parts — `/artifacts/<place>`, then `?f=` for
// the folder you are standing in and `?a=` for the file you have open — and all
// three have to come back together, or returning to the right place with the
// wrong folder is its own kind of lost.

import { viewMemory } from './view-memory'
import type { Place } from '@/routes/app/artifacts'

const PLACE_IDS: readonly Place[] = ['my', 'shared', 'workspace', 'official', 'recent', 'secrets']

export interface ArtifactsSelection {
  place: Place
  /** null = the place's root, which is a real place to be. */
  folderId: string | null
  /** null = nothing open, likewise real. */
  activeId: string | null
}

function parse(raw: unknown): ArtifactsSelection | null {
  if (!raw || typeof raw !== 'object') return null
  const v = raw as Record<string, unknown>
  // A place that no longer exists (renamed or removed in a later release) is
  // not restorable to anything sensible, so the whole memory goes rather than
  // silently relocating someone to `my`.
  if (typeof v.place !== 'string' || !(PLACE_IDS as readonly string[]).includes(v.place)) return null
  return {
    place: v.place as Place,
    folderId: typeof v.folderId === 'string' && v.folderId ? v.folderId : null,
    activeId: typeof v.activeId === 'string' && v.activeId ? v.activeId : null,
  }
}

const memory = viewMemory<ArtifactsSelection>('talaria:artifacts-selection', parse)

export function readArtifactsSelection(): ArtifactsSelection | null {
  return memory.read()
}

export function writeArtifactsSelection(sel: ArtifactsSelection): void {
  memory.write(sel)
}

/**
 * The remembered selection, with the parts that no longer exist dropped.
 *
 * DEGRADES RATHER THAN DISCARDS, which is the difference from Comms. A folder
 * that was deleted does not invalidate the place, and a file that was deleted
 * does not invalidate the folder — each part is independently still useful, and
 * throwing the lot away sends someone to the root of `my` when the place they
 * were in is perfectly fine.
 *
 * Pass `folderIds`/`artifactIds` as null when that list has not loaded: the part
 * is then taken on trust, since discarding it for a list that merely has not
 * arrived yet is the same race in a new place.
 */
export function restorableArtifactsSelection(
  saved: ArtifactsSelection | null,
  rosters: { folderIds: string[] | null; artifactIds: string[] | null },
): ArtifactsSelection | null {
  if (!saved) return null
  const folderId = saved.folderId && rosters.folderIds && !rosters.folderIds.includes(saved.folderId)
    ? null
    : saved.folderId
  const activeId = saved.activeId && rosters.artifactIds && !rosters.artifactIds.includes(saved.activeId)
    ? null
    : saved.activeId
  // A file open in a folder that has since gone would render detached from the
  // browser around it; the folder is the container, so it governs.
  return { place: saved.place, folderId, activeId: folderId === saved.folderId ? activeId : null }
}

/** Tests only: drop the remembered selection and the load latch. */
export function resetArtifactsSelection(): void {
  memory.reset()
}

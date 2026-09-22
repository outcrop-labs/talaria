- **Google Drive is a place in Files, browsed like every other place — the
  import modal is retired.** The rail's Sources row opens the Drive place:
  the same list/grid columns, the same selection and keyboard grammar,
  breadcrumbs into Drive folders (walked server-side so a deep link rebuilds
  its own path), server-side sorting (honest with pagination), and Load more
  past the first hundred. The rail expands the ROSTER while you're in the
  place — one row per browsable Drive, visually distinct: your My Drive,
  shared drives your account joined, and the org connection's Workspace
  Drive marked `org` — so personal and org files can't be confused. The URL
  carries the selection (`/artifacts/drive?d=<drive>&f=<folder>`), the same
  URL-is-the-selection rule as everywhere. Import is a first-class verb now:
  select Drive files, Import, and they land in a My Files folder named for
  the Drive folder they came from — sequential pulls (Google rate limits),
  one summary toast, oversize files counted. New: GET
  /api/integrations/google/drive/drives (the roster; 409 only when BOTH
  connections are absent — that 409 is the Drive place's connect screen) and
  GET .../drive/browse (folders included, paginated, with the walked path);
  the import route gained `folderId`. The old flat /drive/files search route
  is gone with its modal; the agent drive route is untouched.

### Added

# Files

Files holds everything your workspace produces and keeps: documents, spreadsheets, web
pages, uploads — plus a sealed **Secrets** vault. Agents file their output here too (the
**Agents** folder), and anything you mark **official** is mirrored into Knowledge, so it
grounds your agents.

Open: **Work → Files** in the sidebar. Admins can hide Files from individual people
(Admin → People).

## Places

The left rail splits your files by ownership, not by folder — folders only exist inside
**My Files**:

| Place | What's there |
| :--- | :--- |
| **My Files** | Yours — the only place with folders (create, rename, drag between them) |
| **Shared with me** | Documents and files people shared with you |
| **Workspace** | Owned by the organization, not a person — mostly agent output |
| **Official** | Files mirrored into the knowledgebase |
| **Recent** | The last 50 things touched |
| **Secrets** | The vault — see below |

## To…

| Do this | How |
| :--- | :--- |
| Create | **+ New file** → Document, Spreadsheet, Web page — or **Upload files** |
| Upload | Drag files anywhere (**Drop to upload**), or onto a folder to land them there. Up to 25 MB each |
| Find | **Search files**; sort by name, kind, owner, or modified |
| Move | Drag onto a folder — or onto a breadcrumb segment to move up |
| See details | Right-click → **Properties** — location, owner, sharing, identifiers. It reports; it never edits |
| Download | Right-click → **Download** (or the ⬇ button on a public page) |
| Share | Right-click → **Share** — same dialog as Knowledge: people & agents, Restricted / Organization / Anyone with the link |
| Export to Google | Editor toolbar → **Export to Google Docs / Sheets / Drive** (needs your Google connected) |
| Import from Drive | **Sources → Google Drive** → search → import. Lands in Files, not Knowledge |
| Delete | Right-click → **Delete** — a folder's contents move up a level, they are not deleted |

The three kinds: **Document** (rich markdown, versioned), **Spreadsheet** (rows and columns
you add and delete inline), **Web page** (raw HTML, rendered live at its public link).

## Official — the Knowledge bridge

**Make official** (owner) mirrors the file into the knowledgebase, where it grounds every
agent. Un-officializing removes the mirror; edits to an official file re-mirror. It's a real
knowledgebase change, not just a badge.

## Sharing, honestly

Folder sharing is **not** a blanket grant — files inside keep their own sharing; the folder
just sets the default. Publishing to the open web (**Anyone with the link**) is its own
permission your admin may not have given you — without it, that tier simply isn't offered.
Public links (`/a/…`) render documents, tables, and web pages for anyone; files become a
download button.

## Secrets

Credentials, sealed: no preview, no export, no public tier — by design.

| Do this | How |
| :--- | :--- |
| Add one | Name it ("Staging Stripe key"), paste the value, optionally pin the host and a note |
| Organize | Folders are yours; **Move to** from the row menu |
| Share | **Share…** — named people *can reveal it* (every look is recorded); agents *can use it without ever seeing it* |

A secret is never workspace-wide and never public. This is also why credentials never go in
documents or chats — the vault is the one place built for them.

## Who can do what

Creating documents and uploading are on for members by default; publishing to the web is
its own (usually admin) permission. Workspace-owned files are governed by admins.

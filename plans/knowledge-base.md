# Global development knowledge base

## Goal

Build a personal library of reusable development knowledge inside Devcroft. The
user can preserve selected Markdown guidance and images, save public web links,
organize entries into collections and tags, read them on a dedicated page, and
explicitly ask an agent to consult or maintain individual entries or collections.

Knowledge belongs to the person, independently of repositories and spaces. Store
it in `portable/knowledge/` so the existing portable Git backup and sync carry it
between devices. Repository artifacts continue to document repository work.

Representative workflows agreed with the user:

- Copy a useful Go microservice guideline into a new Markdown entry, retaining
  its images and optionally its source. Adapt its structure to the user's needs;
  the saved entry remains useful without the original repository.
- Save a blog post about agent workflows and read the live website inside Devcroft.
- Put style guides, architecture notes, and concurrency lessons in a **Go
  engineering** collection. An entry can belong to multiple collections.
- Ask an agent to review changes against a particular guide, check for a previously
  encountered concurrency problem, or suggest improvements to project X using a
  collection. After a discussion, explicitly ask it to save the useful conclusions.

## Non-goals

- Repository/space ownership, filtering, or automatic project-specific injection.
- A panel beside the agent/editor; the first version has a dedicated Knowledge page.
- Whole-repository or documentation-tree import, recursive crawling, automatic
  source refresh, or preserving arbitrary directory structures.
- Website snapshots, extracted article bodies, offline web reading, website content
  indexing, persistent logins, browser profiles, tabs, or download management.
- Nested collections, knowledge graphs, semantic search/embeddings, an AI chat
  inside the library, automatic recommendations, or automatic note generation.
- Artifact workflow statuses, review comments, or automatic artifact/To Read migration.
- An MCP server in this implementation. The shared store/service is reusable by a
  later MCP adapter; the existing headless CLI is the simpler initial agent interface.
- Linux embedded browsing in the first release. The user explicitly accepted
  macOS embedded browsing with an explicit external-browser action on Linux.

## Files that own this behavior

Existing integration points, verified against the checkout:

| File | Responsibility |
| --- | --- |
| `src/data/mod.rs` | Register the knowledge store and resolve the existing data root. |
| `src/data/record.rs`, `src/data/store_lock.rs` | Reuse bounded reads, atomic writes, revisions, and portable sync coordination; add knowledge locking/ID support without changing artifact identities. |
| `src/data/sync.rs`, `src/data/portable.rs` | Existing portable Git sync and restore already cover the new subtree; add regression coverage. |
| `src/data/spaces.rs` | Verify space rename/removal and legacy migration leave knowledge untouched. |
| `src/cli.rs`, `src/main.rs`, `src/commands/mod.rs`, `src/commands/input.rs`, `src/commands/output.rs` | Register headless commands and reuse input/output conventions. |
| `src/workspace.rs`, `src/home.rs`, `src/command_palette.rs`, `src/navigation.rs` | Dedicated page, Home entry, palette/navigation commands, origin/back behavior, focus, drafts, and page lifecycle. |
| `src/preview.rs`, `src/markdown_references.rs` | Shared Markdown reader, image resolution, knowledge references, local hover details, and reference opening. |
| `src/artifacts.rs`, `src/tutorial_view.rs` | Existing draft/refresh and webview lifecycle patterns to reuse selectively; knowledge remains a separate feature. |
| `src/webview/mod.rs`, `src/webview/accessibility.rs`, `src/webview/input.rs` | Native hosting, composition, keyboard focus, and accessibility; preserve the existing restricted HTML host. |
| `Cargo.toml`, `Cargo.lock` | Only change if native history access or image rendering requires a direct dependency. |
| `assets/skills/devcroft/SKILL.md`, `src/agent_skill.rs` | Teach knowledge discovery and maintenance and embed the new reference guide in the installed skill. |
| `README.md`, `docs/home.md`, `docs/resources.md`, `docs/keyboard-reference.md` | Document access and the distinction between repository artifacts and global knowledge. |

Proposed new owners:

- `src/data/knowledge.rs` and `src/data/knowledge/`: records, collections,
  attachments, search, shared operations, and store tests.
- `src/cli/knowledge.rs`, `src/commands/knowledge.rs`: CLI contract and thin handlers.
- `src/knowledge.rs` and `src/knowledge/`: browser, editor/forms, and public link viewer.
- `src/webview/public.rs`: public-site policy and native navigation adapter.
- `tests/knowledge_cli.rs`, `src/knowledge_browser_tests.rs`: headless and UI state tests.
- `docs/knowledge.md`, `assets/skills/devcroft/references/knowledge.md`: user and agent guidance.

## Approach

### 1. Records, attachments, and shared operations

Use separate knowledge records, rather than adding kinds to repository artifacts.
The proposed layout is:

```text
portable/knowledge/
  entries/<kb-id>/entry.md
  entries/<kb-id>/assets/<asset-id>.<ext>
  collections/<collection-id>.md
```

Use a versioned JSON metadata block between `---` delimiters, followed by Markdown,
following the artifact representation. Each record has one authoritative metadata
and body file. Preserve unknown metadata when updating supported schema versions.

An entry has a stable ID, title, immutable kind (`markdown` or `link`), tags,
collection IDs, creation/update times, optional source information, and an attachment
manifest. A link additionally requires an HTTP(S) URL. Its Markdown body holds the
user's optional notes; a Markdown entry's body is the document itself. There is no
repository or space field. A collection has its own stable ID, name, optional
Markdown description, and timestamps. Membership is stored only on entries.

Collections are flat and many-to-many. Tags are freeform, trimmed, case-insensitively
deduplicated, and retain a display spelling. Local collection create/rename rejects
a name already present under trimmed case-insensitive comparison; entry titles
may repeat. Separate devices can still create or rename collections to the same
name before syncing. Preserve both records after sync, show their IDs when names
collide, and return every match during discovery. Never merge, discard, or select
one automatically by name; agents must resolve ambiguity explicitly. Use IDs for
mutations, membership, and references. Collections and entries can be renamed
without changing identity.
Removing an entry from a collection does not delete it. Deleting a collection
deletes only that collection record: missing collection IDs do not hide entries,
do not count as valid memberships, and are pruned on the next explicit entry save.
This also handles collections removed by another device without a multi-file rewrite.

All desktop and CLI work calls the same typed store operations. Reuse the portable
gate, then a knowledge-wide read/write lock under device-local `cache/`, in that
order. Start with one knowledge lock for understandable entry/collection consistency;
do blocking I/O off the UI thread. Sync retains its exclusive portable gate.
Create IDs with collision checks and separate prefixes for entries and collections.
Updates and deletes require the current record revision; omitted fields remain
unchanged and explicit clear operations are supported. A stale save preserves the
desktop draft and returns a conflict to the CLI. Missing or malformed records
produce useful errors without preventing access to healthy records.

Image attachment behavior:

- Support explicit local image attachment in the desktop and CLI. Copy bytes into
  the entry's assets directory and insert/return a relative Markdown image reference.
  Support PNG, JPEG, GIF, and WebP initially; validate actual supported image data,
  with a documented initial limit of 20 MiB per asset and 4 MiB per record.
- Use immutable asset names and a manifest containing ID, original name, media
  type, byte size, and relative path. Write assets before atomically publishing the
  record that refers to them, under the same gate/lock. A failed record save can
  leave an unreferenced file, but never a committed reference to a partial asset.
- Keep unsaved form attachments in device-local temporary storage. Save copies
  them before publishing the record; cancel removes the temporary draft files.
- Read images through an entry-aware resolver restricted to registered assets in
  that entry. Reject path traversal, symlinks, and arbitrary local paths. Do not
  add host filesystem capabilities to public web pages.
- A single-file Markdown import copies the document body. Offer to attach its
  referenced local images, with explicit mapping for missing/outside-directory
  images; rewrite image destinations using Markdown-aware parsing. Do not copy
  other documents. Remote image URLs stay remote until the user supplies a local
  attachment. Explain unresolved images before committing the import.
- Deleting an entry removes its record first, making it absent from the library,
  then removes its managed assets. Unreferenced attachment cleanup is best-effort;
  it must not delete assets referenced by a committed record. No automatic asset
  garbage collection or in-app version-history UI is needed in the first release.

Reuse the shared reader with a knowledge mode that has an outline, in-document
find, code highlighting, and references, but no artifact comment controls. The
current `PreviewView` does not carry an entry asset base, so image resolution is
explicit work, not an assumption about existing Markdown rendering.

### 2. Search and CLI access

Search titles, Markdown bodies/link notes, tags, saved URLs, and collection names
and descriptions. Search never fetches websites. Start with background enumeration
and a rebuildable in-memory search snapshot; no new persistent index is required.
Use case-insensitive token matching, deterministic ranking favoring title/tag
matches, then a stable ID tie-break. Provide entry kind, collection, and tag filters.
Define and test AND behavior for multiple query terms/tags. An empty query returns
the normal list. All lists use bounded pagination with explicit continuation and
per-record errors, so agents can inspect a large collection without silent omission.

Proposed command families (final flags follow existing CLI conventions):

```text
devcroft knowledge list|search|get|create|update|delete
devcroft knowledge collection list|get|create|update|delete
devcroft knowledge attachment list|add|get
```

- Every command supports `--json` with `formatVersion`, stable IDs/references,
  revisions, and structured results. Discovery never requires a checkout, active
  space, or running desktop. Errors preserve nonzero exit status and existing
  stdout/stderr conventions.
- `get` returns metadata, Markdown content/notes, saved URL when relevant, collection
  references, and attachment metadata. `attachment get` resolves an asset to its
  validated device-local path for a local agent; paths are not portable identities.
- Creation accepts a content file/stdin, title, kind, optional source, URL for link
  entries, tags, and collection IDs. Updates permit changing text and memberships
  with revision checks. Attachment add also checks/returns the entry revision and
  a usable Markdown image reference; it does not silently rewrite the document.
- Collection retrieval returns its description and paginated member summaries;
  agents fetch relevant members individually. Collection list/search supports
  discovery by name. Membership summaries include entry revisions, so an agent
  can recognize changed documents when retrieving them.
- Duplicate URLs are permitted for separately annotated entries. The desktop's
  Save this page flow offers to open existing matches before creating another.

Keep parsing and output in CLI handlers. Domain operations accept plain typed
arguments and return typed results; a later MCP server calls these same operations
without spawning the CLI or depending on the desktop.

### 3. Dedicated Knowledge page

Expose **Knowledge** on Home and **Open knowledge base** in the action palette,
reachable while working in any repository. Use the existing origin/back pattern
to return to the prior workspace without stopping agent or terminal sessions.
The page is global even when the titlebar has a particular space selected.

Layout: collection navigation (All entries, Uncollected, named collections), an
entry list with search/kind/tag filters, and the selected entry's reading/editing
area. A collection has an editable description and a Copy reference action.
Entries expose title, kind, tags, collection memberships, source, and Copy reference.
Provide New Markdown, Save link, Import Markdown, Edit, and Delete actions, along
with collection creation/rename/delete and membership editing. Use explicit save
and cancel for content edits, preserving drafts through navigation and conflicts.
Reuse existing discard/unsaved-work conventions on exit or app close.

Markdown uses the existing source-edit/preview interaction. Link entries offer
**Website** and **Notes** views so annotations remain accessible even when the
website cannot load. A link is saved immediately with the entered title or a URL
fallback; remote metadata fetching is not required to create a usable entry.

Refresh on activation, successful local edits, and portable sync/branch changes.
While active, detect CLI changes with the existing polling/background-refresh
pattern. Preserve stable selection, filters, scroll, and drafts when their record
is unchanged. A changed record reloads only when there is no local draft; otherwise
show a conflict/change indicator. Separate empty, loading, no-results, and error
states. A removed selected entry must not leave a stale editable record displayed.
Space switching/renaming/removal must neither filter nor rewrite this library.

### 4. Embedded public-site reading

Create a separate public-site host. `src/webview/mod.rs::create` currently permits
only `about:blank`/`about:srcdoc`, denies downloads/new windows, and hosts sandboxed
authored HTML with network access blocked. Preserve this behavior for tutorials
and reviews. Reuse only policy-independent native composition/lifecycle helpers.

The public viewer uses ephemeral website storage and normal HTTP(S) navigation,
with no Devcroft IPC bridge or arbitrary local-file access. It shows the current
top-level URL and supports back, forward, reload, and **Save this page**. Public
page scripts and subresources work normally. Unsupported schemes and download
attempts produce a visible explanation; never dispatch them to the shell. Handle
ordinary target-blank HTTP(S) links in the same viewer where supported; otherwise
offer the destination explicitly instead of silently losing the action.

Use native navigation/history state rather than reconstructing history from link
clicks. The pinned Wry wrapper exposes URL, reload, navigation/load/title callbacks,
and a macOS WKWebView handle, but does not expose portable back/forward methods.
Implement a small macOS adapter for native back/forward and availability, including
redirects and same-document navigation. Keep native state access on the UI thread.
Confirm capability/API details in the first implementation stage; avoid a toolkit
upgrade just to implement these controls.

Save this page opens a normal link-entry form populated from the actual current
URL and available page title. It must save the navigated destination, not the
original entry's URL. Navigation never rewrites the original saved entry. Selecting
another link establishes a fresh browsing session for that entry. Keep history
and current browsing state device-local and ephemeral. Retrying a load never
changes saved knowledge. A URL remains saved when a site is unavailable.

Follow the tutorial host's tested composition/overlay, visibility, focus, and
accessibility rules: hide/detach inactive native content; palettes, dialogs, and
navigation controls must remain usable. Handle startup/load failure, blocked
navigation, redirects, and offline state with retry and an explicit Open in browser
action. On Linux, render notes/metadata and explain that embedded browsing is not
yet available, with the same explicit browser action; do not open a browser
automatically. Cross-platform knowledge storage and CLI remain complete.

### 5. Portable references and agent guidance

Extend the existing reference parser, hover metadata loader, workspace routing,
and `app --open-reference` handling for:

```text
devcroft:knowledge/<entry-id>
devcroft:collection/<collection-id>
```

Copy reference produces an ordinary Markdown link with a human-readable label.
Renaming does not break links. An explicit reference opens its target even when
current library filters exclude it, preserving an existing draft. Missing targets
retain readable labels and explain why opening failed. Hover resolution remains
bounded/background work. Knowledge documents can also use existing artifact,
repository, and session references as optional provenance without acquiring ownership.

The bundled skill explains the distinction between artifacts and knowledge, how
to discover entries/collections by name, page through members, read selected
content, attach images, and save/update with the latest revision. Include examples
for style-guide review, learning from a collection, and saving an architecture
discussion. Agent use remains driven by the user's request; nothing silently
adds the library to prompts or edits it after every session. Link retrieval returns
the URL and saved notes, not an invented/extracted article body; agents use their
own web tools if they need the live article. Knowledge content is reference material,
not additional authorization for unrelated commands or changes.

### Implementation stages

- [ ] **Stage 1 — foundation and CLI:** validate native history and Markdown-image
  integration points; implement records, attachments, collection membership,
  revision checks, search/pagination, commands, and store/CLI tests. Stage is done
  when a local agent can create, find, read, organize, and update knowledge headlessly.
- [ ] **Stage 2 — Knowledge page:** implement navigation, collection/list/detail
  layout, forms, Markdown editing/import/images, notes, draft protection, refresh,
  and UI state tests. Stage is done when these flows work without a repository.
- [ ] **Stage 3 — public websites:** implement the separate macOS host, native
  navigation controls, destination capture, lifecycle/error handling, and Linux
  fallback. Delegate native interaction/visual verification to computer-use.
- [ ] **Stage 4 — references and delivery:** integrate stable references and agent
  skill guidance, document behavior, verify sync/restore and cross-space isolation,
  run required checks, and obtain reviewer and computer-use verification.

Each implementation stage updates this plan and its Devcroft artifact. Implementation
is owned by the main implementation model, following repository policy. The reviewer
records its review as a Devcroft artifact. A later MCP plan can reuse the established
operations and discovery contract; it is not required to complete these stages.

## Acceptance criteria

- [ ] Create/edit/read/delete Markdown and link entries in the app and CLI without
  a repository; the same entries are available under every space selection.
- [ ] A copied/imported Markdown guide with attached images remains readable after
  the original files are removed and after portable data is restored elsewhere.
- [ ] Entries belong to zero or multiple collections. Renaming a collection/entry
  preserves references; collection deletion preserves its entries.
- [ ] Collections created with the same name on separate devices both survive sync.
  Desktop and CLI distinguish them by ID, preserve their separate memberships,
  and never silently choose one based on its name.
- [ ] Search matches saved text, titles, URLs, tags, and collections; filters compose
  predictably. Website body text is neither indexed nor claimed to be searchable.
- [ ] Paginated CLI discovery exposes every healthy matching entry and reports
  malformed records. Desktop and CLI share the same content and revision behavior.
- [ ] An agent can inspect a collection, retrieve its members, create a new note,
  and update it explicitly. A stale desktop or CLI update preserves newer content.
- [ ] macOS public websites render inside Devcroft, including images/scripts needed
  for ordinary reading; back/forward/reload work after links, redirects, and anchors.
- [ ] Save this page captures the currently displayed destination with editable
  title/notes/organization, while leaving the original link entry unchanged.
- [ ] Palettes/dialogs remain usable over the viewer; leaving Knowledge hides native
  content and returns to the prior workspace with its processes still running.
- [ ] Linux offers complete Markdown/library/CLI behavior and explicit browser
  fallback for URLs. It builds without requiring macOS-only dependencies.
- [ ] Stable entry/collection links work from artifacts and knowledge, including
  after renaming or switching spaces; missing targets and conflicts are explained.
- [ ] Portable sync/restore includes documents, collection records, and images;
  browser state, locks, drafts, and search caches are excluded.
- [ ] Existing tutorial/review webview restrictions and artifact behavior still pass
  regression checks. Public web content cannot invoke a Devcroft host bridge.

## Verification commands

Planning-session validation: check document structure, owner paths, whitespace, and
independent review. No application build or test result is implied by this plan.

Planning review: [Knowledge base plan review](devcroft:artifact/art-9av4nsz8).
The review's collection-name collision finding was incorporated and re-reviewed;
no unresolved actionable findings remain. Native history and image integration
remain implementation-stage validation tasks.

During implementation, run focused checks after the corresponding stage:

```sh
cargo test --locked data::knowledge::
cargo test --locked --test knowledge_cli
cargo test --locked knowledge_browser_tests::
cargo test --locked markdown_references::
cargo test --locked webview::
cargo test --locked data::sync::
cargo test --locked data::spaces::
cargo test --locked agent_skill::
```

New test/module names above are proposed owners and become runnable when introduced.
Before delivery:

```sh
mise run check
mise run test
cargo build --locked
cargo check --locked --no-default-features
git diff --check
```

Compile/test on the existing Linux CI runner as well. Add targeted fixtures for
concurrent CLI/desktop updates, interrupted attachment publication, malformed
records, deleted collections, same-name collections created on separate devices,
asset path containment, cross-device image restore, and stale async search/navigation
results. Use temporary fixture roots inside
automated tests; do not redirect `DEVCROFT_DATA_DIR` for actual Devcroft records.

Delegate UI interaction and visual verification to the computer-use agent with
bounded scenarios: create a Markdown entry and attach an image; organize it in two
collections; search and follow a stable reference; retain a draft during a CLI
conflict; browse a public page and its references, back/forward/reload, and save the
destination; open a palette/dialog; leave and return; switch spaces; check notes
while offline and light/dark layout. Use available native UI tools rather than
assuming the Codex in-app browser exists. The agent reports observed behavior;
the main implementation model owns fixes. Keep test/review output distilled.

## Risks / open questions

- **Public webview history and native lifecycle:** the largest implementation risk.
  Validate the macOS adapter early, especially redirects, fragment/SPA history,
  failed loads, focus, and overlays. A working load-URL demo alone is insufficient.
- **Markdown image support:** verify an asset-aware rendering path against the
  pinned GPUI Kit APIs. Preserve source Markdown and existing reader behavior.
  SVG import and clipboard-image paste can follow the initial explicit attachment flow.
- **Public sites vary:** some require authentication or fail in an embedded viewer.
  Preserve saved URLs and notes and provide retry/external open; do not promise
  universal website compatibility or offline article content.
- **Portable Git growth:** attached images increase backup size; use bounded,
  immutable assets and explain failures. Content extraction and automatic cleanup
  are outside this first implementation.
- **Collections across sync:** missing/deleted collection IDs must not hide entries.
  Independent same-name collections survive as distinct IDs and require explicit
  disambiguation. Existing Git conflict handling still applies; local revision
  locks do not solve conflicting edits made on different devices.
- **Search size:** begin with a background local scan/snapshot and pagination.
  Measure realistic libraries before adding a persistent index or semantic search.
- **Product decisions resolved:** global scope; dedicated page; Markdown plus live
  links; portable attachments; flat many-to-many collections and tags; explicit
  agent reads/writes; CLI first; macOS embedded viewer first. No product decision
  blocks the staged implementation described above.

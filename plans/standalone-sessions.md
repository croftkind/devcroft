# Chat page and tagged agent threads

Status: implemented and reviewed. Required automated checks and delegated native UI checks for the core flows passed. Narrow-window layout remains unverified.

[Interactive concept](devcroft:artifact/art-zr9jsvfs) · [Plan in Devcroft](devcroft:artifact/art-6hw752h4) · [Design review](devcroft:artifact/art-kyq2a2c5) · [Implementation review](devcroft:artifact/art-zkn4q7gw). Local concept: `plans/standalone-sessions.html`.

## Goal

Provide a dedicated **Chat** page for machine investigations, ideas, system design, and other work outside a repository. Organize its threads with multiple tags and a grouped sidebar. Reuse Devcroft's harness picker, terminal, activity indicators, spaces, and navigation mode. Keep repository Agent sessions in their existing workflow.

User preferences confirmed during exploration: **start immediately in a Devcroft-managed working folder; optionally choose a folder**; use a dedicated **Chat** page; create via navigation mode with **space first, then harness**; allow **multiple tags, with the same thread appearing in several groups**. General is the default tag.

## Non-goals

- A common message renderer or transcript format across harnesses. Chat is the page name; its conversation area still hosts the selected harness.
- Fake Git repositories, extra projects in the project picker, or a replacement project model.
- A global redesign of navigation or a second dashboard section for the same recent sessions.
- Automatic import of every conversation on the machine.
- Synchronizing harness transcripts, credentials, working folders, or local paths.
- Changing harness permissions or introducing a permission-free “discussion mode.”
- Cross-harness continuation of the same conversation.

## Files that own this behavior

Implementation ownership:

- `src/home.rs`, `src/home/dashboard.rs`: Home navigation and existing Recent Sessions section.
- `src/workspace.rs`: page selection, repository panes, current working directory, space switcher, keyboard dispatch, and `OpenAgentSession`.
- `src/workspace_sessions.rs`: harness picker, launch/resume, sidebar, native identity matching, and Home session projection.
- `src/agent_sessions/model.rs`: native `SessionKey`, working directory, inferred checkout, resume arguments, and validation.
- `src/agent_sessions/mod.rs`, `src/agent_sessions/providers.rs`: disposable device-local catalog/cache and provider metadata discovery.
- `src/agent_accounts.rs`: shared or isolated accounts, explicit space ownership, captured provider stores, and resolved launch contexts.
- `src/pane.rs`, `src/session.rs`, `src/agent_activity/`: retained PTYs, shell startup, activity tracking, and native session signals.
- `src/data/spaces.rs`, `src/data/device.rs`, `src/settings.rs`: space lifecycle, local state, and account configuration guards.
- `src/command_palette.rs`, `src/navigation.rs`: entry points, contextual actions, and keyboard hints.
- `src/main.rs`, `docs/home.md`, `docs/keyboard-reference.md`: registration and documentation.

Added `src/data/chat.rs` for the durable local thread/tag registry and `src/chat.rs` for the dedicated page, creation flow, filtering, and retained terminals. Repository and Chat launches share the existing terminal and activity machinery through a resolved launch context.

## Approach

### Product placement

**Chat** is an app-level destination, alongside Home and the existing full-page destinations. It is not a repository tab or a virtual project. The underlying threads remain standalone agent sessions owned by a space.

- Use the existing navigation prefix plus **`o` — Open Chat** from every ordinary page context. The default prefix is Cmd+J on macOS and Ctrl+J elsewhere; respect the configured prefix. `o` is currently unused. `c` already means Add comment in Resources; h/j/k/l are reserved for navigation movement. Do not repurpose those keys.
- Add **Open Chat** and **New chat** to the actions palette. A single **Chat →** link in Home's existing Recent Sessions section gives mouse discoverability. No new global tab strip or dashboard section is needed.
- Home's recent cards can include Chat threads with a Chat/space label, while repository sessions keep their project label and open their existing workspace. Keep the current card limit.
- In Chat, navigation mode **`n`** opens New chat, **`s`** switches space, **`f`** focuses thread search, **`t`** opens tag filters, **`b`** returns to the prior page, and **Space** goes Home. Navigation mode h/l moves between sidebar and terminal; j/k traverses visible groups/threads; Enter toggles a group or selects a thread. Plain typing outside navigation mode continues to reach the focused harness.
- Repository Agent retains its existing navigation `n` and New session behavior. New chat is a distinct global action and always targets Chat.
- Opening Chat restores its browsing state and any retained live pane; it never starts a process just to display the page. Respect existing unsaved-editor/Git navigation guards and return origins. Do not intercept shortcuts while a modal dialog owns input.

Chat's header shows Back, **Chat**, the space selector, and existing command/navigation affordances. The sidebar contains search, a compact tag-filter control, a **Tags / Recent** view switch, collapsible tag groups, and **New chat** at the bottom. Rows show title, harness, activity/time, and space in All. The main area hosts the harness with a compact thread header for title and editable tags. There is no repository tab strip, branch badge, Git polling, or Resources pane. Reveal working folder stays in the thread actions.

### Starting a chat

1. Enter navigation mode and press **`n`** on Chat (or use New chat / its global palette action).
2. Open one dialog, initially on **Space**. Preselect the active named space; from All use the last concrete creation space, otherwise the first catalog space. Show it visibly. Arrow keys change the space; Enter accepts the selection and advances. All is a browsing filter, never a creation choice.
3. In the same dialog, show **Harness**, with the chosen space summarized above the existing enabled-harness list. Preselect the normal default harness. Arrow keys select; Enter starts. Back returns to Space; Escape cancels the dialog without creating a record or process. Tab/Shift+Tab use normal dialog focus traversal.
4. Keep **Working folder: Managed by Devcroft** collapsed on the harness step. Choose folder remains optional. Do not require a title, prompt, or tag before launch.
5. Create the local record and folder, launch the harness, and show the new terminal. Every new chat begins in **General**. Switch Chat to the chosen space and clear any search/tag/harness/archive filters that would hide it; expand General and select the new row. Make filter changes visible. Prompt entry happens inside the harness as today.

Accepting the defaults takes two Enter presses: one to confirm the preselected space, one to start the preselected harness. This explicitly follows the requested space-then-harness flow. A dialog state machine separates choosing space, choosing harness, launching, failure, and cancellation; disable repeated submission while launching and ignore held-key repeats so moving from Space to Harness always requires a second physical Enter press to start.

### Tags, groups, and filtering

- Tags are **many-to-many**. A thread can have `ideas` and `system-design`. In Tags view it appears in both groups; every row points to the same thread ID, live pane, native identity, and archive/title state. Cross-listing must never create another conversation.
- **General** is a reserved fallback label for threads with no custom tags. Adding the first custom tag removes it from General; removing the last returns it. General cannot be renamed or deleted and is not combined with custom tags. No mandatory categorization step interrupts creation.
- Default to **Tags** view: collapsible tag groups, threads sorted by last activity inside each group, tag groups sorted by name with General last. Remember view mode and collapsed groups per browsing space. **Recent** is a flat, deduplicated list sorted by last activity, with compact tag chips.
- Show a unique thread count for the overall result. Each group's count describes that group's matching threads; counts across groups are not additive. In Tags view, all visible copies of the selected thread reflect selection; repeated selection focuses the same live pane.
- Search matches thread titles and tag names, not full transcripts. Tags filter uses **match any selected tag** (OR); combine it with space, search, and optional harness/status filters using AND. Put secondary filters in a compact popover, rather than a row of permanent controls. Archived is off by default.
- Filtering by one or more tags shows only those tag groups. Recent view still deduplicates. Empty states distinguish no chats yet from no filter matches and offer Clear filters.
- Edit tags from the thread header or row menu: searchable multi-select, create a tag inline, and remove assignments. Tag edits are Devcroft metadata; they never rewrite provider transcripts. Archive, rename, and tag changes update every visible copy immediately.
- Tags are local to a **space**, with stable tag IDs and case-insensitive, trimmed unique names. `ideas` and `system-design` can be suggested initially; do not prepopulate a long empty taxonomy. Rename a tag updates all its thread labels. Deleting a tag removes only its assignments, preserving chats and applying General where needed.
- All-space browsing may merge equal normalized tag names into one visual group/filter; rows keep their space badges. Such a filter matches the union of those spaces' distinct tag IDs. Tag-management actions operate on a specific space; they cannot silently rename/delete matching tags in other spaces.
- Changing the space filter does not move any thread or change its account. Detach a selected pane that is outside the chosen space while keeping its process alive; restore a suitable already-live pane or show a selection prompt. Changing only search/tag filters can leave the selected chat visible with **Outside current filters** and Clear filters. Always display actual owner space in the thread header.
- On same-account space reassignment, map custom tags to destination tags by normalized name, creating missing ones in the same atomic metadata operation. Cross-account reassignment remains forbidden as specified below. No tag operation changes account or folder.

Each default session receives a persistent, unique folder such as `<data-root>/session-workspaces/<session-id>/`. It is outside the checkout and `portable/`; it is not a Git repository or a temporary directory. The UI shows a friendly name and Reveal folder, not an internal path by default. A managed folder organizes files; it does not enforce a sandbox.

The space chooses the account. The folder chooses the working directory. A chosen folder does not silently change either session ownership or credentials. If it is inside a registered repository, offer the ordinary **Start in project** path. Explicitly continuing as standalone keeps the chosen space and shows that the folder has project context. Harness instruction discovery and permissions still apply normally.

Folder and harness are fixed for a conversation. Changing them means a new session; resuming must use the original context. A missing folder produces a recovery state, not a silent fallback to Home or the last checkout. The first release can offer Reveal location and Start new session rather than promising provider-safe relocation.

### Thread history and lifecycle

- Show title, harness, activity/relative time, and space where useful. Reuse current running/waiting/attention behavior.
- Rename is a local title override, preserving provider metadata. Include title search and archive to keep the list useful over time. Archived sessions have a discoverable filter and can be restored.
- Closing a live session ends its process but retains history and files. Archiving hides a stopped session; require closing it first. Archiving never deletes the provider transcript or a user-selected folder.
- Switching space, visiting a repository, or returning Home preserves live panes. Selecting an already-open session focuses it rather than spawning a duplicate.
- After application restart, display saved history without launching all previous processes. Resume only on selection.
- Keep rows whose native history disappeared, with **History unavailable** and a way to archive them. Do not quietly turn Resume into New.
- Scope the Chat sidebar to the active space. All aggregates its threads with space labels. Repository Agent histories stay outside this sidebar; Home recent cards may show both kinds.
- Store only sessions created through this flow in the first release. A later explicit import flow can let users assign selected external sessions to a space.
- Supported launch choices remain consistent with existing Settings. Current catalog discovery/resume covers opencode, Codex, and Claude; `SessionSummary::validate` excludes omp. Until an omp adapter exists, label it **Launch only — resume unavailable** and retain its local row honestly.

### Data and launch model

Repository discovery currently infers `checkout` from `cwd`, and the sidebar filters by that checkout. Keep that behavior for existing history. Add explicit ownership for standalone history instead of teaching discovery that every unregistered directory is a project.

Suggested model boundaries (names illustrative):

```text
SessionOwner = Repository(repository_key) | Standalone(local_session_id)

StandaloneSessionRecord
  id, space identity, harness
  working_directory, managed_directory flag
  account reference / shared-account marker
  native_key?: { provider, store, id }
  title_override?, tag_ids[], created_at, last_activity_at, archived_at?
  binding state: pending | bound | unavailable | unsupported

ChatTag
  id, space identity, name, normalized_name

ChatViewState (device-local)
  space selection, view mode, filters, collapsed groups, selected thread ID

AgentLaunchContext
  owner, cwd, resolved account, optional native resume key
```

The registry is durable device-local data under a dedicated path such as `local/chat.json`; the catalog remains a rebuildable cache. Keep tags, assignments, rename/delete operations, and same-account moves atomic within the local registry. Tag IDs/assignments belong to durable metadata, not the disposable provider cache. Use the existing locking/atomic-write conventions. Failure to read or persist ownership must stop that launch, not fall back to repository or shared-account behavior. Create the record before spawning so a crash cannot erase its space and folder association. Startup reconciles pending records and interrupted launches without auto-starting them.

Use one resolved launch context throughout new/resume, account preparation, activity tracking, permission checks, history filtering, and live-pane validation. Add explicit account lookup by space; do not reuse `for_checkout` for standalone launches. Extend idle guards so changing isolation accounts also considers standalone panes. Native keys retain their provider store, so the same native ID in two accounts cannot collide.

Native identity is a release gate. Existing code uses provider signals and a unique-candidate fallback based on provider, checkout, and time. A unique managed directory helps, but timestamp proximity alone cannot safely bind custom-folder sessions. During implementation, verify per-provider launch identity capture and restart recovery. Persist an authoritative identity when available; use only demonstrably unique candidates otherwise. On ambiguity, keep a pending row and offer explicit selection from eligible candidates; never attach the newest conversation arbitrarily. This matters for concurrent sessions in the same folder, shell sessions launched outside Devcroft, and crash recovery. If a provider cannot meet that contract, expose its limitation or defer custom-folder support for it.

Do not use a mutable space name in folder paths. Add an opaque stable ID to space records if needed for local references, preserve it through renames and sync, and leave existing repository name-based references intact. This is an additive space-identity change, not a repository migration. Migration must persist IDs atomically and reject duplicates. If two devices independently assign different IDs to the same pre-existing space, sync must surface reconciliation and keep unmatched local sessions under an **Unavailable space** state; never reassign accounts by name silently. An upstream deleted space also leaves recoverable rows with their original account association. Explicit space removal must count standalone history and block while its sessions are live. Stopped history may be reassigned only to a destination using the same account/store. Refuse reassignment across accounts; offer a new session in the destination while retaining the original history with its original account reference in an unavailable-space recovery/archive view. Do not present that recovery history as resumable under the destination credentials. Removing the source space must not erase its referenced local account data. Transcript stores are not moved implicitly. Resolving these rules is part of the data phase, not optional polish.

Existing repository sessions need no bulk import or rewrite. Add ownership in the application projection and the standalone registry. Reuse existing provider parsers, shell quoting, PTY lifecycle, and terminal UI. Do not mutate provider databases to rename or archive local records.

### Alternatives considered

- **Virtual project per space:** initially reuses the repository workspace, but pollutes project lists and invites Git, worktree, relationship, and Resources behavior. Useful only if it becomes a real general-purpose workspace concept later.
- **Global Agent tab:** direct access, but the existing tabs belong to the open repository. Adding an app-wide tab there makes scope ambiguous and changes every repository screen.
- **Persistent workspace/room:** useful for repeated work with shared files and instructions. Adds naming and lifecycle decisions before a simple conversation. Consider later as an optional group over sessions when repeated use demonstrates a need.
- **Chat page with tags:** selected direction. It follows the app's full-page destinations, adds keyboard access, and organizes conversations without a project or room hierarchy.

### Phases

- [x] Inspect current source and delegate UI inspection.
- [x] Confirm immediate start with an optional folder.
- [x] Draft the initial standalone-session concept and implementation handoff.
- [x] Refine the direction into Chat with multiple tags, sidebar groups, and space-first keyboard creation.
- [x] Implement safe provider identity capture, non-Git launch contexts, optional custom folders, and account isolation. Validate data rules and fixture launch/resume; expose explicit linking when no authoritative signal is available.
- [x] Implement durable ownership, space identity/lifecycle, folder creation, and recovery; add focused data tests.
- [x] Separate launch context from checkout lookup and add reliable binding/resume; preserve repository behavior.
- [x] Add the Chat page and reuse terminal/sidebar/picker behavior; wire Home, palette, navigation context, and the two-step creation dialog.
- [x] Add local tags, grouped/deduplicated projections, filtering, and atomic tag management.
- [x] Add title search, local rename/archive, unavailable-history states, and documentation.
- [x] Complete final automated verification and record it with the independent review and delegated UI results.
- [ ] Verify narrow-window layout when reliable native window resizing is available.

## Acceptance criteria

- A user with zero registered repositories can select space and harness and start an agent session.
- Navigation-prefix + `o` opens Chat across ordinary page contexts without conflicting with existing commands; `n` opens New chat only in the correct context.
- The dialog presents Space first, with a real space preselected, then Harness. Enter twice accepts defaults and starts once; Escape cancels from either selection step.
- No folder, project, title, prompt, or tag field is required before starting. New chats start in General and remain visible even if earlier filters excluded them.
- Each default session has its own persistent non-Git working directory. No provider-specific bypass of approvals is introduced.
- Choosing a folder is optional; directory and selected space remain visible and do not silently affect each other.
- Space/account ownership survives app restart, local space rename, and sync changes without credential fallback. Browsing another space never changes the owner of an existing conversation.
- Space removal cannot move native history across accounts. Original stores remain recoverable, and creating a session in a destination account always creates a new conversation.
- Two sessions using the same harness and custom folder remain distinct, including across different spaces/accounts.
- Ambiguous native identity cannot resume another conversation; pending and unavailable states remain actionable.
- Supported native history resumes the exact provider/store/id and original working directory. Unsupported resume is labeled before launch.
- Home, repository, and standalone navigation preserve live sessions and show appropriate activity indicators without duplicate processes.
- Existing repository Agent behavior, history, project picker, Git, Review, and Resources continue to work.
- The new destination has no repository tab strip. Home reuses its existing Recent Sessions area and does not acquire another permanent navigation bar.
- Rename, archive, and close have distinct effects; user-selected folders and provider transcripts are never deleted by archive.
- A thread with two tags appears in both groups but uses one ID, native identity, and live pane. Recent view and overall counts deduplicate.
- General fallback, multi-tag OR filtering, combined filters, tag rename/delete, and All-space tag names behave as specified. No tag mutation changes ownership or credentials.
- Group collapse/view/filter preferences restore without automatically launching threads.
- Sessions, tags, working folders, and local provider stores stay outside portable sync.
- Keyboard-only creation, space selection, sidebar navigation, resume, and return-to-Home work; long titles and narrow windows remain usable.

## Verification commands

Required implementation checks:

```sh
mise run check
mise run test
git diff --check
```

Add focused tests for registry atomicity/recovery, ownership/account selection, space lifecycle, ambiguous bindings, concurrent same-folder launches, archive semantics, missing native history, and repository regressions. Include tag projection/deduplication, General fallback, OR/AND filters, tag lifecycle, account-preserving space moves, and navigation-key uniqueness across all resource states. Update existing expectations that leave `o` unused and restrict `n` to the repository Agent context. Exercise dialog focus, two-step Enter, repeated submit, cancellation, and filter reset after creation. Run relevant filters during development and the full required checks once the implementation is ready.

Delegate native UI verification to the computer-use agent: navigation-prefix + o from Home and repository; Chat n → Space → Harness → launch; cancellation at each step; tag assignment and cross-listed row selection; grouped/Recent views; combined filtering; All-space badges; General fallback; retained sessions; restart and exact resume; narrow layout and keyboard focus. Use harmless fixture sessions, not real diagnostic actions against the user's machine.

## Implementation and verification record

The Chat page is implemented with space-first creation, a unique managed working folder by default, optional custom folders, retained terminal panes, tags, grouped and Recent projections, search and combined filters, local rename/archive, and Home/palette/navigation entry points. Settings and page transitions refresh browsing scope without changing thread ownership. Chat activity remains separate from repository activity and Git polling pauses while Chat is visible.

`local/chat.json` holds durable device-local ownership and metadata behind locking and atomic writes. Space IDs survive renames; duplicate/conflicting identities fail explicitly. Space deletion stages Chat changes in the existing rollback transaction, blocks live sessions, and only moves stopped history when the persisted account and provider store match the destination. Missing or conflicting spaces retain recovery records. Provider discovery includes captured stores after environment changes. Ownership read failures stop launch/resume.

Native identity uses authoritative provider signals where available. A pending thread offers **Link history** with eligible candidates rather than guessing from recency. Resume validates the exact provider/store/ID and original working folder. OMP is launch-only. Opening Chat or restarting the application never starts a process automatically.

Independent source review is recorded in [art-zkn4q7gw](devcroft:artifact/art-zkn4q7gw). All identified ownership, account/store, rollback, navigation, activity-projection, and asynchronous lifecycle findings were corrected. The final pass also corrected reopening a retained Artifacts page from Chat and preserving nested Back targets. Chat now captures and restores Home's destination, scroll position, and polling state; a GPUI regression covers Artifacts, Relationships, and dashboard roundtrips. No unresolved source correctness findings remained after the final source pass.

Delegated native UI verification used a temporary app bundle and harmless provider fixture, isolated from the user's normal Devcroft data. Passed observations: Home's Chat link and empty state; mouse and keyboard creation; Space then Harness with two physical Enter presses; held Enter launches once; Escape cancels either step; General fallback; two tag groups sharing one thread and terminal; deduplicated Recent view; search with no results and Clear filters; tag filtering and clearing; retaining the live pane through Home; explicit history linking, close, and exact native-ID resume; and no automatic resume after restart. These checks establish fixture behavior, not authenticated end-to-end behavior of every installed provider. Narrow-window layout remains unverified because the available UI controls could not reliably resize the test window.

Final automated results on the completed implementation:

- `mise run check`: passed formatting, locked compilation, and clippy for all targets with warnings denied.
- `mise run test`: 862 unit tests passed, 7 ignored; all 8 CLI integration tests passed; no failures. This includes the final nested-page and dashboard return regression.
- `git diff --check`: passed.

The mise commands used `MISE_OFFLINE=true MISE_AUTO_INSTALL=false` to avoid unrelated global tool resolution. The complete test suite ran outside the sandbox so its existing localhost TCP fixture could bind a port. The native fixture tests and source review do not establish authenticated behavior across every provider; those limits and the unverified narrow-window layout remain explicit above.

## Risks / open questions

- Authenticated end-to-end behavior across every provider remains outside the fixture verification. Providers without authoritative launch identity require explicit history linking; OMP has no resume adapter.
- Stable space identity is migrated and validated. Independently assigned conflicting IDs require reconciliation; local history remains recoverable instead of being reassigned by name.
- A folder can contain repository instructions even when the session is standalone. Label the context; do not suggest “standalone” removes harness access or instructions.
- Per-session working folders need an eventual explicit cleanup flow. The first release keeps files when archiving and offers Reveal folder.
- Portable notes are a useful later extension: **Save summary to space** would let ideas outlive local harness history. Artifacts are currently repository-owned, so space-owned artifacts are a separate change.
- A companion Terminal or Files view may help machine investigations. Start with Reveal folder; add these only if use shows frequent exits from the harness.
- Later enhancements: pinned threads, optional named rooms, explicit external-history import, and starting a project session from a Chat summary. Tags should be enough for the first organization model.
- Chat describes the destination and threads; its center remains the selected harness. A shared chat bubble UI would be a separate provider-integration project.
- `o` opens Chat and is documented in the navigation HUD and keyboard reference; the existing `c` comment command remains available in Resources.

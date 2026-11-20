# Devcroft v2 architecture plan

## Goal

Make a workspace the place to discover related work and a task the place to deliver it. A developer can investigate a change across several repositories, promote that conversation into a task, implement in isolated worktrees, and review the resulting diffs and pull requests from the same task page.

Build and test **Devcroft Next** alongside the installed Devcroft. “Next” is a proposed preview name. The developer must be able to run either app, or both at once, throughout development. Start with a local Rust server and a native NSH desktop with TypeScript/TSX screens. Use a small Rust module for local IPC and native components where measurements justify them. GPUI is the fallback if a required native interaction proves impractical.

The first daily-use milestone should support this workflow with Codex, Claude, and Antigravity in one native desktop interface. Keep provider choice, model, effort where available, permission requests, and questions visible in context. Preserve fast keyboard navigation while making common actions discoverable on screen.

The product scope comes from `docs/devcroft-v2.md` and the subsequent design discussion. The pinned study in `docs/t3code/` explains the provider and orchestration ideas behind the design. In particular, a Devcroft task is the user's deliverable. A provider run, a delegated assignment, and a Git worktree each have a different lifetime.

**Start here:** implement [the first change](#first-implementation-change), check it against [the coexistence contract](#running-both-versions), and run [the bootstrap verification](#verification-commands). The remaining milestones describe the path from that change to daily use.

## Non-goals

- Existing data import, CLI compatibility, or reproducing the portable Git sync format. V2 can use a fresh store. Use a separate store during development so both versions can be run independently.
- Remote connections, same-thread provider switching, OpenCode, and cross-provider delegation in the first daily-use milestone. Keep each as a distinct follow-up.
- A second embedded editor, a web frontend, and a GitHub Issues integration as prerequisites. The issue analogy describes the task interaction.

## Files that own this behavior

These are the current owners to consult when extracting behavior or deciding what to replace. The service and task model below are proposed.

| Current files | What the implementation needs from them |
| --- | --- |
| `src/workspace.rs`, `src/workspace_sessions.rs`, `src/navigation.rs`, `src/command_palette.rs` | Repository selection, session navigation, shortcuts and focus. Rebuild navigation around workspace, task and thread identities. |
| `src/agent_accounts.rs`, `src/agent_sessions/`, `src/agent_activity/` | Account environment setup, native conversation references, existing harness discovery. Structured adapters will supply run activity. |
| `src/data/`, `src/cli.rs`, `src/commands/` | Repository relationships, portable and device stores, CLI entry points. Establish new workspace ownership and route shared operations through the host. |
| `src/git/`, `src/review/`, `src/pull_requests.rs` | Checkout operations, diff and line anchors, GitHub status. Scope reads and actions to the task checkout or reviewed PR revision. |
| `src/session.rs`, `src/editor/`, `src/terminal_graphics.rs` | PTY and terminal rendering, built-in editor and navigation. Separate host file and process access from rendering. |
| `src/artifacts.rs`, `src/data/artifacts.rs`, `src/tutorial_view.rs`, `src/webview/`, `src/tools/` | Artifact discovery and feedback, interactive HTML rendering, developer tools. |
| `Cargo.toml`, `mise.toml`, `Packager.toml`, `scripts/install-app.sh`, `scripts/install.sh`, `packaging/`, `.github/workflows/` | Stable build, installer and release identities. In particular the local install script replaces the stable macOS bundle, relinks the CLI, and cleans up legacy Linux entries. Start Next with its own build and packaging entry points. |

**Repository decision.** Start implementation in a sibling Git repository named `devcroft-next`. Independent dependencies, packaging and release jobs let the developer update the stable app throughout a substantial rewrite. Keep the server and desktop together in Next so a protocol change and its clients can ship in one commit. Begin locally. Remote hosting, visibility and eventual cutover can be decided later.

Keep this plan at `plans/devcroft-v2.md` here as the planning handoff. At Next bootstrap, copy the reviewed plan and its design inputs (`docs/devcroft-v2.md` and `docs/t3code/`) to the same relative paths in Next. Record the source commit or plan artifact revision and maintain implementation progress in Next's copy. The study's relative code links and the current-owner table above refer to the original checkout. When porting code, record its source commit and carry over its license and applicable asset attributions.

```text
devcroft-next/
  Cargo.toml, Cargo.lock              independent workspace and dependency pins
  crates/protocol/src/lib.rs         wire types, IDs, handshake and version
  crates/host/src/main.rs            devcroft-next-host entry point
  crates/host/src/paths.rs           preview root, endpoint and ownership checks
  crates/host/src/{ipc,store}.rs     server dispatch, migrations and transactions
  crates/host/migrations/           numbered SQLite migrations
  crates/client/src/lib.rs           connect, request, subscribe, reconnect
  desktop/{App.tsx,nsh.json}         NSH entry and preview package identity
  desktop/src/lib.rs                mixed-template native module for IPC
  desktop/screens/                  workspace, task, reply and review views (TSX)
  scripts/                          launch and package the matching host and client
```

Add workspace, task, run, provider, terminal and GitHub modules to the host as their slices land. Keep wire types free of NSH, GPUI, SQLite and native SDK types. The Rust client shares the protocol crate. The desktop module exposes a small typed interface to TSX, with generated types or fixtures checked against Rust serialization in CI. Keep SQLite and provider SDK dependencies on the server side. Start with Rust/Zig versions from stable's `mise.toml`, then pin working Rust, Zig, NSH CLI and crate revisions in Next's own tool configuration.

## Approach

### First implementation change

**Deliver a second app that can identify and reconnect to its own host.** This comes before implementing agents, Git operations, or the full task screen. Use the current macOS installation for the first native smoke test. Add Linux host and protocol tests to Next's CI using the same Linux runner family as stable.

1. Add the nested workspace, preview path resolver, and the protocol's `hello` request. Use a temporary preview data root in tests. Generate a persistent host ID in that root. Two invocations with the same root must identify the same installation.
2. Implement `devcroft-next-host serve` and `devcroft-next-host status --json`. The latter connects as a client. It reports channel, protocol version, host ID, and connection status. Acquire one process lock per canonical root and bind a private socket. Test two simultaneous launches and two independent temporary roots.
3. In the Next checkout, run `nsh init desktop --template mixed`. Keep the generated TSX entry and Rust module registration. Set `nsh.json` to name `Devcroft Next` with ID `com.devcroft.desktop.next`. Have the Rust module connect off the UI thread and return a typed handshake to TSX. Show connecting, disconnected and connected states, with the host ID in a diagnostics view. Package the matching host executable with the client. [Mixed app template](https://nsh.dev/docs/getting-started/) · [Package metadata](https://nsh.dev/docs/packaging/)
4. Supply a development launcher that builds the host, gives the desktop the exact host path, and opens Next. The packaged client resolves the helper relative to its own installation. The client attempts a handshake first, starts the helper only when no host is listening, and waits for readiness. An incompatible live host produces an actionable version error. Quit detaches the desktop. Provide a preview-specific host stop operation for testing and upgrades.
5. Demonstrate stable + Next side by side, then quit and reopen Next against the same host. A second isolated Next test instance gets a different host ID. Ship this change with the commands and fixtures listed under Verification. The following change can proceed headlessly with a scripted provider while the representative NSH screen is exercised.

### Running both versions

| Surface | Current installation | Next preview contract |
| --- | --- | --- |
| macOS application | `Devcroft.app`, ID `com.devcroft.desktop` | `Devcroft Next.app`, ID `com.devcroft.desktop.next`. Open directly from the preview output or install under its own name. Give the window and About panel the preview name. |
| Linux and command line | `devcroft`, stable launcher and icon | `devcroft-next` for the preview launcher or future CLI, `devcroft-next-host` for the service, `devcroft-next.desktop` and matching separate icon and window identity. Use distinct filenames even on case-insensitive filesystems. |
| Durable files | Stable per-OS `devcroft` root or `DEVCROFT_DATA_DIR` | Sibling `devcroft-next` root under Application Support on macOS or XDG data home on Linux. An absolute `DEVCROFT_NEXT_DATA_DIR` selects a disposable or alternate preview root. Keep DB, assets, logs, account homes and locks preview-owned. Reject a preview root overlapping the effective or default stable store, including symlink aliases, before creating files. |
| IPC and local preferences | Stable app and its existing integrations | Scope singleton locks, socket names, keychain service names, window preferences and notification identity to Next. Include a hash of the canonical preview root in short socket paths under a private per-user runtime directory. Bind with owner-only permissions. A stale endpoint may be reclaimed after checking the lock and live listener. |
| Source repositories | The user's existing checkouts and active sessions | Clone workspace repositories into a fresh directory chosen for Next. Use fresh task worktrees from those clones. A clone URL or local source may identify the same project. Adoption, account setup and skill installation must never write into stable's checkouts, provider configuration or skill locations as a side effect. |
| Development, install, update | Stable Cargo project, install scripts and release workflow | Launch and stage from the Next checkout. Preview packaging copies the server and any SDK helper into its own package, signs the finished macOS bundle, and limits install/uninstall to preview-owned paths. Preview builds use their own update channel if one is introduced. Removing a preview binary leaves its data available for reinstall. |

Test installers in a temporary install prefix containing a sentinel stable bundle, launcher, CLI and data files. Compare their contents and link targets before and after preview install, upgrade and uninstall. Also open the real stable app beside the **staged** preview for a smoke test. Automated tests use disposable data and repositories. A new preview store starts empty. Profile onboarding can reuse the same paid subscriptions through separate verified provider instances.

### Ownership

| Record | Ownership and behavior |
| --- | --- |
| Profile | Owns configured provider instances, subscription or credential references, skills, and optional instructions. Track usage by instance when the provider exposes it. Resolve identity on the execution host. |
| Workspace | Belongs to one profile. Has a dedicated directory with its own clones, workspace instructions and skills, and an agent-generated index of repositories and relationships. Store when the index was generated and support refresh. Resolve profile and workspace guidance for every agent launch, including launches in task roots. |
| Task | Belongs to one workspace. Has a title, description, user-visible state, one durable parent thread, and links to checkouts, artifacts, and PRs. Completing an agent turn leaves the task available for the next decision. |
| Thread and run | A thread owns its transcript. A run records one execution with its provider instance and native conversation reference. A reply thread also records the originating thread and message. Questions and approvals retain the run and request identity needed to deliver an answer. |
| Artifact and PR | Discoverable at workspace level. Explicit links identify which artifacts a task produced or used, and which PRs it contributed to. A workspace can also discover PRs for which there is no Devcroft task. |

### From exploration to delivery

1. A workspace chat starts with visibility into all its repositories. The agent can inspect files, consult the relationship index, brainstorm, and propose changes. Use a discovery execution policy that keeps source edits in tasks.
2. **Create task from chat** preserves the conversation and references as the parent thread. Record its new task binding and a task-specific execution root before starting implementation. If the provider cannot resume across the directory change, start a new native conversation with the relevant history. A task can also start directly with a title and description.
3. Give each task a directory with a stable relative path for each workspace repository. Start with one Git worktree per repository under that directory, capturing each base commit. This is a proposed simple first implementation. Measure preparation cost before choosing lazy creation. Two tasks touching the same repository get separate branches and working directories. Make setup retryable after a partially created set of worktrees.
4. The task page shows the description, state, current summary, and parent conversation. Start a discussion from the task or reply to a particular message. Child threads share the task checkouts, can see uncommitted changes, and remain individually addressable. Return a concise outcome and a link when a discussion resolves. Make that outcome available to the parent's next turn, including after reconnect, with one delivery per outcome.
5. File links, the built-in editor, per-repository terminal tabs, and the grouped diff open against the selected task. Keep workspace-root terminals available. Switching tasks selects the corresponding terminals and preserves each live shell's working directory. Compare each checkout with its recorded base and include staged, unstaged, and untracked work. Retain dirty worktrees when the user marks a task done.

For the first implementation, allow one write-capable agent run at a time per task. If a child thread is editing and the parent receives another implementation request, show which thread is writing and queue the new run. Read-only discussions can continue when the adapter can enforce read-only access. Hold the writer slot while a process can still edit, including while it awaits approval. Reconcile its process state before releasing the slot after a host restart. This policy coordinates agent runs sharing dirty files within a task.

### Native desktop and local host

**Proposed deployment:** the desktop starts or attaches to a local Rust server. The host owns workspaces, Git and filesystem access, provider processes, PTYs, and durable records. The client owns rendering, navigation, focus, and input. Use typed commands, snapshots, and resumable event subscriptions over local IPC. Identify a file by host, workspace or task checkout, and relative path. Apply the same boundary to terminal I/O, artifact assets, and editor saves so a future remote client can use it.

Start with a local transactional store such as SQLite for tasks, transcripts, links, pending requests, and command receipts. Store rich artifact payloads and assets on the host with stable IDs and revisions. Keep provider credentials in host-side profile configuration. Reconnect to an existing host after desktop exit. Record a prompt and its run identity before dispatch. If the host itself dies after dispatch, reconcile the native run where possible and surface an interrupted or uncertain outcome when necessary. Blind replay could execute a code change twice.

Use NSH TSX screens for the workspace and task UI. Keep a typed client-side projection per open thread, page older messages, update the changed timeline item, and batch streaming deltas to a render frame. Keep parsing, Git queries and large file reads off the UI thread. The terminal has a separate high-volume stream. Start with the host's libghostty state and visible-row updates to a native terminal surface. Keep cursor, input methods, selection, resize, paste and scrollback in the native test. Rendering an HTML artifact uses a restricted embedded web view. It has no ambient access to the native IPC module. Test macOS and Linux keyboard focus as the user moves among thread, code, terminal and web view. On Linux, verify GTK and WebKit packaging on the target distro. [NSH native components](https://nsh.dev/docs/native-components/) · [NSH Linux web view configuration](https://nsh.dev/docs/rust/)

**Proposed responsiveness targets.** In a release preview, load a persisted 5,000-item thread, stream 30 text deltas per second into the last item, and keep a terminal receiving output. During a fixed 30-second scroll and typing sequence, target p95 frame time at most 20 ms on a 60 Hz display and p95 key-to-visible-character latency at most 50 ms in the composer and an echoing local terminal. Open a cached task from the workspace to its first readable screen within 250 ms p95 over 30 switches. Record machine, OS, refresh rate, build flags, run count and measured percentiles. Check that scrolling back preserves the viewport and streamed updates preserve a selected code range. These are initial engineering budgets for the user's machine. Diagnose missed budgets by measuring host work, IPC, TSX updates and native presentation independently. Optimize the slow path, then compare GPUI on the same fixture only if NSH still misses an essential interaction or budget.

**Task page interactions.** The workspace opens on its task list with visible entries for discovery chat, repositories, artifacts, and PRs. Open a task to see its title, workflow state, current summary, conversation, changed repositories and linked PRs. The composer identifies whether the next message starts a discussion or continues a selected reply thread. A source message exposes “Reply in thread” and a reply count. Follow a thread and return to the originating message with keyboard focus and scroll position restored. Keep tool output expandable; show pending questions and approvals inline with a direct inbox link. Reopening the task should make outstanding decisions visible without expanding every discussion.

### Host contract and recovery

Start on macOS and Linux with length-prefixed UTF-8 JSON frames over a Unix domain socket. Validate frame sizes and protocol major version before decoding a payload. The mixed-template Rust module owns socket I/O, cancellation and reconnect. The TSX screen consumes snapshots, deltas and connection status. Keep transport behind the client so a future SSH connection can carry the same messages.

For bootstrap, use a four-byte unsigned big-endian length, a 1 MiB frame limit, and a request ID echoed in success or error replies. Paginate large reads and chunk asset transfers. Proposed first exchange, shown without the length prefix:

```jsonl
{"id":"probe-1","method":"hello","params":{"protocol_major":1,"channel":"next"}}
{"id":"probe-1","result":{"protocol_major":1,"channel":"next","host_id":"host-123"}}
```

Here `host-123` represents the opaque ID generated on first launch. The status probe must never start a host itself. Test partial frames, unknown methods, version mismatch, EOF and two in-flight requests whose replies arrive out of order.

| Operation | Minimum request and result |
| --- | --- |
| Handshake and read | `hello` returns channel, protocol version and host ID. `workspace.get` returns task summaries, repository index metadata and pending attention counts. `thread.get` returns a bounded page of ordered timeline items, active runs, pending requests and a cursor. |
| Subscribe | `subscribe(scope, after_seq)` returns committed events after the cursor. A snapshot includes its high-water mark from the same read transaction. On a gap or expired cursor obtain a fresh snapshot. Use durable item IDs to discard duplicates. Slow consumers can resnapshot. |
| Create or promote | `task.create` takes workspace ID, title and description. `task.promote` also takes the source thread and expected revision. Both return a task ID and preparation status. The client can observe progress even if Git setup fails partway. |
| Conversation | `thread.reply` takes parent thread, optional source item and prompt. `run.start` captures prompt, provider instance, model/options, checkout binding and write mode. `run.cancel` targets one run. `request.answer` identifies the originating request and attempt. `thread.resolve` commits the outcome and a parent timeline reference. |
| Files and tools | A file reference carries workspace ID, checkout ID, relative path and optional blob or revision. Read, save with expected revision, diff, terminal open/attach/input/resize, and artifact fetch all resolve on the host. A stale editor save returns a conflict and keeps the draft. |

Use a unique client-generated command ID for each durable mutation. In one SQLite transaction validate scope and expected revision, store the command receipt, update records, append ordered events, and queue any side effect. A repeated ID with the same payload returns the prior result. Reject reusing the ID with different input. Broadcast only committed events. Return stable error categories for missing scope, stale revision, unsupported capability, unreachable provider, or interrupted run. Surface the next available action in the relevant thread.

For a run, keep a durable lifecycle of queued, dispatching, running, waiting for input, and completed/failed/cancelled/interrupted. Record the attempt before provider dispatch. On host restart, restart work known to be undispatched, reconcile a known native run, and flag ambiguous delivery for inspection. Discard or attribute late events using attempt and native turn IDs. Store pending questions and approvals with their native request ID. After loss of the provider process, mark an unanswerable request expired. Keep terminal I/O and transient token chunks out of the durable event ledger. Periodically checkpoint streaming text and persist the final timeline item.

### Persistence and worktree recovery

Build the schema in migrations as each feature lands. Enable foreign keys and use opaque, host-generated IDs. Initial tables should support these constraints and lookups.

| Tables | Keys and invariants |
| --- | --- |
| `profiles`, `provider_instances`, `workspaces`, `workspace_repos` | Instance and workspace each reference a profile. Store driver, account reference, checkout root, clone URL and unique relative checkout name under their owners. Verify a run's account belongs to its workspace profile. Keep secrets out of transcripts and the DB. |
| `tasks`, `task_checkouts` | Track user state (todo/in progress/done) separately from preparation (preparing/ready/failed). Unique checkout per task and workspace repo. Store canonical worktree path, branch, pinned base OID and phase. Scope queries by workspace and task. |
| `threads`, `timeline_items`, `thread_outcomes` | Workspace discovery, task parent/reply, or profile general chat. A unique parent per task, replies within the same task, stable source item ID, ordered sequence per thread. Outcomes have revision, source and destination. Updating a decision appends a superseding outcome. |
| `runs`, `attempts`, `native_conversations`, `runtime_requests` | Persist selection and execution root per attempt. A native reference belongs to a provider instance, app thread and compatible checkout context. Enforce one active agent writer per task transactionally. Deduplicate native event/request IDs within their scope. |
| `receipts`, `events`, `pending_effects` | Reconnect reads by scope and sequence. A receipt links a command to its result. Claims and dispatch state distinguish replay-safe operations from uncertain native turn starts. |
| `artifacts`, `artifact_revisions`, `task_artifacts`, `pull_requests`, `task_prs` | Task links stay in the same workspace. Record producer/consumer role. Store artifact selection with revision and excerpt. Identify GitHub PR by repository and number, review anchor by head/base and diff side. |

Keep stable relative repo names across the workspace clone directory and each task directory. For example, discovery sees `<workspace>/repos/api` and `repos/client`. A task launches at `<task-root>` and sees `repos/api` and `repos/client` backed by its own worktrees. Place sibling task roots outside the discovery checkout tree. Make the workspace index and resolved instructions available at the appropriate launch root. Resolve references by their typed scope and relative path. Validate actual filesystem destinations before saving. Constrain provider subprocesses separately. Shared Git metadata for linked worktrees requires an explicit policy for staging and commits, for example host-mediated Git operations. A working directory alone cannot enforce this boundary.

**Promotion algorithm.** Finish or cancel active discovery work before rebinding the thread. In one transaction reserve the task ID, source thread, repository membership and per-repository target refs. Resolve the current committed HEADs once and persist every base OID before materializing worktrees. Use a generated branch such as `devcroft-next/<task-id>` in each independent repository. Base on the recorded commits. Disclose existing uncommitted workspace changes in the promotion UI. Leave them in the discovery clones. As each worktree succeeds, record its identity. On retry compare Git's actual worktree path, branch and base with the reservation. Reuse matching checkouts and report conflicts at the affected repository. When all are ready, atomically make the existing thread the task parent and enable implementation. A failure leaves discovery history accessible and allows retry by task ID. Keep the source-thread reservation unique under concurrent clicks.

**Thread context.** Start a reply with the task brief, originating message and current decisions. Let it read further history by item ID. On resolution, persist the outcome and parent link together and record which revision has been included in a parent run. An idle parent receives it on the next turn. Queue updates that arrive during an active turn. A decision survives a closed child thread, app restart or a change of model.

### Provider implementation order

Use a scripted adapter for deterministic host tests. Then implement Codex end to end, and exercise the other two integration paths early enough to learn where the contract differs. Each adapter supplies discovery/health, account binding, supported model and effort choices, start/resume, send, cancel, answer-request and a normalized event stream. Store raw diagnostic payloads only with sensitive fields excluded. Record sanitized protocol fixtures for the orderings that matter.

| Adapter | First evidence to collect |
| --- | --- |
| Codex | Use a profile-owned app-server process. Verify create, resume, turn start/interrupt, tool activity, approvals and late completion. Start under the selected account and task directory. Test that discovery credentials or runtime options cannot accidentally carry across promotion. |
| Claude | Use the Agent SDK through a small version-pinned helper if necessary. Test helper packaging on a clean machine, streaming, permissions/questions, cancellation and account identity under the user's subscription. Translate SDK events at the helper boundary. |
| Antigravity | Probe `agy acp --help` for the installed version, initialize ACP over stdio, and verify prompt, streamed updates, permission response, cancel and session continuity. Stable's `src/agent_accounts.rs` skips a custom home for this provider. Verify a supported way to isolate its mutable account state in Next. Report an unavailable configuration explicitly if isolation cannot be established. [Google's editor integration guide](https://antigravity.google/docs/ide/extensions/acp/) |

Probe the account, sandbox and permission capabilities actually offered by each installed version. An adapter may expose exploration first while its write isolation is being proven. The daily-use gate requires the three providers to complete a real task within scope and use the intended subscription. A simulated pass cannot establish that.

### Delivery sequence

- [ ] **0. Bootstrap the independent preview.** Deliver the first implementation change above in the Next repository. Exit when the staged native app and its Rust host work alongside stable and the coexistence tests pass.

- [ ] **1. Prove the boundaries.** Exercise one real turn, native permission or question handling as supported, interruption, and profile selection for each initial provider. Evaluate Codex app-server, Claude Agent SDK with a narrow host-side bridge if needed, and Antigravity ACP. Verify task write boundaries through actual provider file and shell operations, including attempts through symlinks or paths outside the task. Check account separation using two profiles for one provider. Build the representative native screen and demonstrate reconnect to one live host run. [T3Code study](../docs/t3code/01-unified-agent-runtime.md) · [Antigravity ACP](https://antigravity.google/docs/cli/acp/)
- [ ] **2. Deliver one end-to-end task with one provider.** Wire profile and workspace setup, index generation, discovery chat, promotion, worktree preparation, parent and reply threads, outcome delivery, built-in editing, shell access, and grouped diff. Use a two-repository change as the acceptance fixture. Include a second task that touches one of those repositories.
- [ ] **3. Reach daily use.** Complete the three provider integrations with capability-aware controls. Add workspace task and artifact discovery, Markdown and interactive HTML artifacts with selection feedback, rich chat references, images, tables, and Mermaid. Link task PRs, show their checks, and surface pending requests and run completion in the inbox. Exercise the entire workflow with each provider under its intended subscription.
- [ ] **4. Complete the wider product.** Deliver workspace GitHub review with author, participation and state filters, watched PR notifications, comments, approval, and agent discussions anchored to selected diff lines. Add a dedicated general chat area with a profile and scratch directory, a global human- and agent-editable wiki with search and references, and a visible tools destination for JSON, diff, and Base64 utilities. Expose available usage per subscription. These features can be scheduled independently after the task workflow is usable.
- [ ] **5. Extend orchestration.** Add same-thread provider switching with a bounded, attributed history handoff. Keep the original transcript and run attribution. Add OpenCode through the same adapter contract. Later, give a parent agent an explicit cross-provider delegation operation with a scoped assignment, child identity, and durable result. A future remote host can implement the existing client contract after transport, authentication, and disconnect behavior have been designed.

## Acceptance criteria

- Launch stable and Next simultaneously from distinct bundles or launchers. Read an existing stable record with the installed stable CLI while Next performs a disposable task. Restart or upgrade Next and verify both launchers, stable records and active stable sessions remain usable. Use the fixture installer test for exact before/after comparisons.
- Meet the native scrolling, typing, terminal echo and cached-task budgets on a recorded reference machine, or document the measured failing interaction and fix it before promoting the preview to daily use.
- Discovery can read two related repositories. Promotion retains the conversation once, gives the task a parent thread, and directs subsequent implementation to the task's worktrees. A failed promotion can be retried without duplicating the task or abandoning dirty work.
- Reply to one implementation decision, resolve it, then ask the parent to implement. The parent receives the decision with its source link. Opening the reply shows its full history. Both threads see the same uncommitted changes.
- Start a write-capable child run, pause it for approval, and submit an implementation request in the parent. The parent request stays queued across desktop reconnect and runs once after the child releases the checkout. Another task can continue editing its own worktrees throughout.
- Run two tasks against an overlapping repository. Their terminals, edits, and grouped diffs resolve to the correct checkouts. The main clone and the other task stay clean of those edits. Attempts by an agent to edit repositories outside its workspace fail.
- Close the desktop during a turn and reconnect. Show the completed result or the same pending approval. Submitting the answer once resumes the correct run. Restart the host during an uncertain provider start and verify that the command is never executed twice by an automatic retry.
- Use each initial provider through the same task UI. A work profile and a personal profile use their respective accounts. Unsupported controls are presented according to negotiated capabilities.
- Find a design artifact from its producing task, a consuming task, and the workspace. Select a passage or an element in a versioned prototype and return to that selection from its discussion. Open both PRs for a two-repository task. In the review milestone, post a comment, verify it on GitHub, and receive a watched PR update.

## Verification commands

**Bootstrap commands to add in the Next checkout.** These are deliverables for milestone 0. Stable's `mise run install` invokes the replacement installer. Run the preview from its own checkout using the launch task below.

| Command or location | What a passing result demonstrates |
| --- | --- |
| `cargo fmt --all -- --check`, `cargo test --workspace --locked` at the Next root | Preview path resolution, protocol round-trip and host identity tests pass. Tests use temporary directories. Commit Next's own lockfile. |
| `nsh check App.tsx`, `nsh test App.test.tsx` under `desktop/` | The TSX entry type-checks against its native module and the initial screen handles connection success and failure. Generate the module declarations with the pinned NSH toolchain before checking them. |
| `mise run dev` at the Next root (new task) | Build the matching debug helper and start the mixed-template NSH desktop. Status in a second shell via `cargo run --locked -p devcroft-next-host -- status --json` identifies the same host. |
| `mise run package-preview` (new task) | Build and stage the preview locally. On macOS verify the final bundle identity using PlistBuddy or plutil and its signature with codesign; verify the bundled helper works when the checkout and build tools are off PATH. On Linux inspect the desktop entry and launch the staged binary. |
| `mise run test-coexistence` (new task) | Exercise install, preview upgrade, restart and uninstall against disposable stable and preview destinations. Compare sentinel file digests, symlink targets and process identities. Test an inherited stable environment override, overlapping roots, long socket paths and concurrent start. |

**From milestone 1 onward:**

- Add a scripted provider fixture to host integration tests. Drive prompt, partial output, pending request, answer and completion over the actual socket. Reconnect after each state, retry the same command ID, and kill the fixture between dispatch and acknowledgment. Check for one user message, the expected attempt status, and one writer.
- Seed a disposable workspace with committed `api` and `client` repositories. Start tasks A and B. Fail worktree creation in the second repo for A, retry, and verify the first repo retains its branch and base. Modify both repos in A and the same file independently in B. The UI and terminals must report the expected changes in each scope.
- For the two-task fixture, inspect each repository with `git worktree list --porcelain` and each checkout with `git status --porcelain=v1 --untracked-files=all`. Compare the task's displayed diff against those checkout states and the captured base commits.
- Run the native task-page, terminal, artifact, and reconnect scenarios through the desktop. Use protocol fixtures for deterministic provider failure cases and live authenticated smoke runs for the actual release integrations. Exercise GitHub write actions in a dedicated test PR.

## Risks / open questions

- **Host lifetime.** Start with explicit preview host shutdown and keep the service alive through desktop quit. Decide on idle expiry and background PR watching once there is usage evidence. Remote transport and multi-device sync are separate decisions.
- **Worktree cost and lifecycle.** Measure large workspaces. Define base-branch selection and explicit cleanup for finished tasks with unmerged commits, dirty files, or open shells. Worktrees provide Git isolation. Enforce filesystem access separately for provider tools and shell children.
- **Conversation continuity.** Validate provider behavior on workspace-to-task rebinding, capability discovery, profile-specific auth, and resume. A transcript must remain readable even when the native provider cannot resume.
- **Rich content.** Verify untrusted HTML prototype rendering and stable selection anchors across artifact revisions. Give embedded content an explicit capability boundary before allowing interactive prototypes to exchange messages with the host.
- **Rollout choices.** The separate repository, preview name, IPC encoding, NSH mixed template, numeric performance targets, eager worktrees, and one agent writer per task are proposed defaults for starting implementation. Revisit each against its acceptance fixture. Portable backup or sync can be reconsidered against a concrete need for export, restore, or multiple devices.

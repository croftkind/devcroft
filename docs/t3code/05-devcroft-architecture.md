# Implications for Devcroft’s redesign

Devcroft can use one GPUI conversation surface for its coding agents by introducing a local agent service with provider adapters. The service owns the thread and run records. The selected provider owns its native conversation, credentials, and execution protocol. That distinction lets Devcroft preserve a thread while switching model or account, and lets one agent assign a separate review or implementation task to another.

## Current Devcroft boundary

The Agent workspace opens a `TerminalPane` for the selected harness. Session discovery returns a `SessionKey` identified by provider, store path, and native ID. Provider readers source recent sessions from harness-specific state or RPC, and account isolation wraps harness commands with the configured environment. Activity can be inferred from terminal output and provider-specific evidence. This already gives Devcroft useful provider discovery, account mapping, and keyboard-first navigation. The native session key represents a harness conversation and can become a reference on a Devcroft-owned thread. [Agent workspace](../../src/workspace_sessions.rs) · [Session identity](../../src/agent_sessions/model.rs) · [Session readers](../../src/agent_sessions/providers.rs) · [Account isolation](../../src/agent_accounts.rs) · [Terminal activity](../../src/agent_activity/terminal.rs)

## Proposed service boundary

~~~mermaid
flowchart LR
    UI["GPUI conversation and agent sidebar"]
    CLI["Devcroft CLI"]
    MCP["Devcroft MCP"]
    S["Local agent service"]
    DB["SQLite"]
    W["Workspace and Git services"]
    A["Provider registry and adapters"]
    P["Codex app-server, Claude SDK, other runtimes"]
    UI --> S
    CLI --> S
    MCP --> S
    S --> DB
    S --> W
    S --> A --> P
    P -->|"Provider events"| A
    P -->|"Agent tool calls"| MCP
~~~

The service can begin inside the existing GPUI process behind an asynchronous Rust interface. Give the interface typed commands, snapshots, and subscriptions so it can move to a local daemon if execution must survive app exit or a remote client needs it. The process boundary is a deployment decision. The service boundary establishes ownership immediately. A native provider can still run a binary or managed sidecar under an adapter, while the built-in Terminal continues to run an ordinary shell. [T3 environment ownership](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/docs/internals/overview.md) · [Provider adapter contract](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/packages/provider-core/src/server/ProviderAdapter.ts)

| Proposed record | Minimum useful contents |
| --- | --- |
| Thread | Devcroft-generated ID, repository and checkout binding, selected provider instance/model/options, title, and optional parent reference |
| Provider instance | Stable ID, driver, local credential or home reference, enabled state, and capability/catalog snapshot |
| Native conversation | Thread ID, instance ID, native reference, continuation identity, and a coverage watermark for material delivered |
| Runtime session | Transient provider process or connection, native session capabilities, and enough state to reconcile process loss |
| Run and attempt | Thread and request IDs, resolved selection and workspace, attempt ID, native turn correlation, execution status, and timestamps |
| Timeline item and request | Stable item ID, order, role or activity type, content or reference, and the provider request ID for a live question or approval |
| Handoff and task | Source and destination identities, included item references or assignment, status, and durable delivery acknowledgement |

Model the provider surface around catalog and capabilities, session open and close, native conversation creation and resume, turn start and stop, runtime request response, and normalized events. Add optional operations as the product exposes them. Version-check capabilities when native protocol support can vary. Keep secrets and subprocess handles local and make the UI consume purpose-built projections for thread list and details. [T3 identities and capabilities](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/packages/contracts/src/orchestrationV2.ts) · [Provider integration guide](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/docs/internals/adding-a-provider.md)

For a durable prompt submission, commit the prompt, run intent, and command ID together, then send it to the provider. On process loss, distinguish work that can be reconciled from a provider start whose outcome is unknown. Bind late events to a run attempt and native IDs. Keep the settled result and last usable transcript queryable independently of the live process. A small outbox or a persisted start record can provide the relevant recovery semantics. [T3 event sink](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/apps/server/src/orchestration-v2/EventSink.ts) · [T3 effect recovery](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/apps/server/src/orchestration-v2/EffectOutbox.ts#L142)

## Switching and delegation

**Switching.** Change the target selection on the existing Devcroft thread. Send the original user prompt separately from a bounded selection of attributed history. An initial implementation can create a new native conversation at a switch. Once resume is verified for a provider, retain a native reference and delivery watermark per conversation and send missed history when returning. Preserve full app history for readback, correlate the handoff with the attempted delivery, and surface recovery clearly when a native resume cannot proceed. [T3 switching and handoff details](02-provider-switching.md)

**Delegation.** Expose a thread-scoped MCP capability that reuses the same thread launch and turn services as the UI. The request carries a target provider, model, explicit assignment, workspace strategy, and permissions bounded by the caller. Persist the child and its relationship before launching it. Return a task ID immediately. A first release can show progress and results in GPUI and provide a task-result tool. A durable parent wake then lets the originating provider consume results even after its original turn ends. A review assignment can use the shared checkout to see dirty changes. Parallel implementation can use a worktree with an explicit base commit. The review role is an assignment and requires provider-enforced restrictions if Devcroft promises read-only execution. [T3 delegation details](03-cross-provider-delegation.md)

**Review outcome.** A delegated reviewer can return findings and file references in its child thread. Devcroft can expose the result in the parent timeline and link it to the existing Review surface. A separate write into Review comments should use the existing review model with a repository, branch pair, and valid line anchors. Keeping the source task and Git reference in the link lets a user see which diff the reviewer inspected. [Devcroft review comments](../review-comments.md) · [Devcroft resources](../resources.md)

## Workspace and existing features

| Devcroft surface | How a typed agent service fits |
| --- | --- |
| Agent and session sidebar | List Devcroft threads with the current provider, last executed provider where it differs, run state, and pending questions. Adapt pinned-session and recent-session navigation to the thread ID. |
| Account isolation | Reuse the account and configured store concepts as local provider-instance references. Include account or home identity when validating a native resume. |
| Git and Review | Attach each run to a known checkout and optional Git base. Use existing diffs and review navigation. Add checkpoint capture and coordinated undo only when the scope and provider support can be verified. |
| Editor and Terminal | Route file links, diff links, and working-directory links to the current workspace. Preserve existing editor and shell interactions independently of agent chat. |
| Resources and CLI | Keep repository records and review tooling available to agents. CLI or MCP commands for structured conversations can call the agent service and carry a caller scope. |

For migration, retain business identities for repositories, resources, and review comments. Existing session references identify the provider, store, and native ID. One option is to expose old harness sessions as legacy links until the user opens or imports one. Verified transcripts can be imported with their original metadata, then continued through a portable handoff or a provider-supported native resume under the matching account. Make imports idempotent for a given source and preserve the provenance for pinned sessions and artifact origin links. [Devcroft session model](../../src/agent_sessions/model.rs) · [Devcroft artifact and origin links](../resources.md) · [T3 Code migration example](04-workspace-and-platform.md#legacy-migration)

## Decisions for the redesign

| Decision | Starting point | Evidence to gather |
| --- | --- | --- |
| Agent service lifetime | Local service library inside GPUI with an explicit async boundary | Whether runs must outlive app exit or be attached from another client in the first release |
| Initial integrations | Codex app-server and Claude Agent SDK as two protocol shapes, with a small Rust-to-SDK sidecar if required | Supported native protocol versions, SDK packaging, startup and shutdown behavior, account isolation, and usable approval modes |
| Storage and portability | Local SQLite for thread and run data, with stable IDs for repository and checkout references | Which transcript and artifact references should sync across devices, and how to represent a missing local checkout |
| First switching behavior | Portable history across providers, then native resume and delta handoff | Resume compatibility by account and runtime, historical role fidelity, and behavior at the context limit |
| First delegation behavior | Local child execution, explicit task and isolation, user-visible result, and durable result acknowledgement | Whether first-release workflows need active parent wake, parent-side approval and question routing, or worktree cleanup policies |
| Additional providers | Add one adapter through the same contracts once Codex and Claude exercise the lifecycle | Whether ACP or a dedicated protocol is the better route for OpenCode and other supported harnesses |

Before expanding to remote or autonomous scheduling, exercise a Codex → Claude → Codex transcript, a Claude parent delegating a Codex review of an uncommitted diff, a child in a worktree, interrupted provider start and retry, and a late child result after parent settlement. Use replay fixtures from each provider to verify native event ordering, and use Devcroft’s existing workspace and review entry points to verify navigation. [T3 Code replay testing approach](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/docs/internals/adding-a-provider.md#tests)

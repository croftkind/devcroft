# Unified agent runtime

T3 Code moves the agent interaction out of the terminal and into a typed execution service. The workspace host retains provider credentials, process handles, Git access, and filesystem authority. The web client, Electron renderer, and mobile app consume authenticated environment RPCs. A desktop installation can bundle a local server and use the same boundary as a remote client. [Architecture overview](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/docs/internals/overview.md) · [RPC contracts](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/packages/contracts/src/rpc.ts)

~~~mermaid
flowchart LR
    UI["Web, desktop, or mobile"]
    RPC["Environment RPC"]
    MCP["Thread-scoped MCP"]
    O["Orchestrator"]
    DB["SQLite projections, receipts, events, outbox"]
    W["Effect worker and session manager"]
    A["Provider adapter"]
    P["Native agent runtime"]
    UI --> RPC --> O
    P --> MCP --> O
    O --> DB
    DB --> W --> A --> P
    A -->|"Normalized events"| O
    DB -->|"Snapshots and updates"| RPC
    RPC --> UI
~~~

## Drivers, instances, and adapters

| Layer | Responsibility | T3 Code examples |
| --- | --- | --- |
| Driver kind | Defines provider configuration and behavior plus how to construct an integration. Client-safe definitions carry settings schema and presentation data. | Codex, Claude, Cursor, OpenCode, Pi, ACP Registry |
| Provider instance | Configured installation or account under a stable instance ID. Owns resolved configuration, environment, account state, enablement, and model catalog. Multiple instances can use the same driver. | Two Codex profiles with distinct homes or credentials |
| Model selection | Records the provider instance, model, and provider-specific options. | An instance ID, a model ID, and a selected effort |
| Adapter | Implements the server-facing execution contract and translates native messages into normalized records and events. | Codex app-server adapter, Claude Agent SDK adapter, shared ACP adapter |

The instance registry reconciles configuration into scoped integrations. It decodes configuration through the driver, resolves configured secrets on the host, and can expose an unavailable provider snapshot when setup or configuration fails. The orchestration adapter registry resolves the adapter by **instance ID** and exposes continuation metadata. Driver packages can depend on the narrow `ProviderHost` boundary for host services without importing the server application. [Instance registry](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/apps/server/src/provider/ProviderInstanceRegistry.ts#L204) · [Adapter registry](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/apps/server/src/orchestration-v2/ProviderAdapterRegistry.ts#L41) · [Provider host](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/packages/provider-core/src/server/ProviderHost.ts) · [Provider integration guide](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/docs/internals/adding-a-provider.md)

The protocols underneath vary substantially:

| Provider path | Native integration | Result for the shared interface |
| --- | --- | --- |
| Codex | Structured app-server requests, including `thread/start`, `thread/resume`, `turn/start`, `turn/steer`, and `turn/interrupt`. Supported versions can inject historical items. | A session can service multiple native threads. The adapter correlates native IDs and normalizes notifications, approval requests, tool activity, and turn completion. |
| Claude Code | Claude Agent SDK `query()`, SDK session IDs for resume, and SDK calls for operations such as session fork. | The adapter owns the query lifecycle and maps SDK messages, permissions, questions, and native child work into the common records. Its capability declaration differs from Codex’s. |
| ACP agents | Shared ACP adapter plus registry entries or small provider-specific flavors where behavior requires it. | A generic adapter can cover multiple agents, while provider support and available settings are still reported honestly. |
| Other native integrations | Packages such as OpenCode, Pi, and Cursor implement provider-specific transport behind the same adapter surface. | The orchestrator and UI keep using model selection, common commands, and normalized events. |

[Codex adapter](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/apps/server/src/orchestration-v2/Adapters/CodexAdapterV2.ts#L264) · [Claude adapter](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/apps/server/src/orchestration-v2/Adapters/ClaudeAdapterV2.ts#L552) · [ACP adapter](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/packages/provider-acp/src/server/adapter.ts) · [Provider integration guide](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/docs/internals/adding-a-provider.md)

The adapter contract distinguishes session operations from turn operations. The orchestrator can open and stop a session, ensure or resume a native thread, send a turn, interrupt or steer, answer a runtime request, and receive a native event stream through the adapter. Optional hooks and capabilities cover operations such as native history injection, forking, rollback, model changes, and background work. Capability flags inform orchestration policy as well as UI availability. Model and settings options can also depend on the live session. Closely related providers can differ on details such as whether a runtime mode can change in place. [Adapter contract](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/packages/provider-core/src/server/ProviderAdapter.ts) · [Capability schemas](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/packages/contracts/src/orchestrationV2.ts#L403) · [Runtime policy](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/apps/server/src/orchestration-v2/RuntimePolicy.ts)

## Conversation and execution identity

| Record | What it identifies | Why the distinction matters |
| --- | --- | --- |
| App thread | Durable project conversation and user-visible identity, with workspace binding, model selection, lineage, and active provider-thread pointer | The transcript and user links survive provider changes and process restarts. |
| Run and run attempt | One app-level unit of execution and the identity of a specific attempt | A provider retry or continuation can be tied back to the original execution intent. |
| Provider session | Scoped runtime/process record with settings and negotiated capabilities | Sessions can be pooled, replaced, or detached. Their capabilities can vary at runtime. |
| Provider thread | App-side record for a provider’s native conversation, including instance, native reference, and handoff coverage | A returning provider can resume its own history after a different provider contributes to the app thread. |
| Provider turn | Native execution correlated to a provider thread and app run/attempt | Run settlement, native turn completion, and late events can each be attributed correctly. |
| Node, turn item, and runtime request | Execution structure, timeline activity, and a question or approval addressed to a live runtime | The UI can render messages, commands, files, and child work, and route a user response back to its origin. |

[Conversation and run schemas](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/packages/contracts/src/orchestrationV2.ts#L472) · [Session and native thread schemas](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/packages/contracts/src/orchestrationV2.ts#L996) · [Provider event union](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/packages/provider-core/src/server/ProviderAdapter.ts#L83)

## From a prompt to a transcript

1. The client submits a typed command with a command ID. The orchestrator checks the current projection and records the user message and execution intent, including the selection captured for the run.
2. The event sink commits domain events, projection updates, the accepted command receipt, and queued effects in a single database transaction. Publication follows the commit. A client acknowledgement therefore means the command was recorded. Provider start and execution have their own state.
3. The effect worker claims the operation. The session manager resolves the adapter, obtains a runtime, and ensures or resumes its provider thread. The turn service sends the prompt using the resolved workspace and runtime policy.
4. The adapter emits normalized, correlated events. The provider event ingestor persists the meaningful conversation and execution updates. A command-output stream can send transient chunks to interested clients while the final tool output lands in a durable turn item.
5. The provider turn and app run can settle while the checkpoint and workspace refresh continue. The run-finalization service owns those subsequent operations independently.

[Orchestrator](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/apps/server/src/orchestration-v2/Orchestrator.ts#L7031) · [Event sink](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/apps/server/src/orchestration-v2/EventSink.ts) · [Effect worker](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/apps/server/src/orchestration-v2/EffectWorker.ts) · [Session manager](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/apps/server/src/orchestration-v2/ProviderSessionManager.ts) · [Event ingestor](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/apps/server/src/orchestration-v2/ProviderEventIngestor.ts) · [Run finalization](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/apps/server/src/orchestration-v2/RunFinalizationService.ts)

## Recovery and streaming

An accepted command ID gives the server a durable receipt for retries. That guarantee applies to command acceptance. After a process loss, the outbox distinguishes replay-safe work from effects bound to the lost runtime. A provider turn start, a restore, and a UI reconnect have different recovery rules. Tests cover cases such as receipt races, process loss, and late provider work. [Outbox recovery classifications](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/apps/server/src/orchestration-v2/EffectOutbox.ts#L142) · [Provider runtime recovery](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/apps/server/src/orchestration-v2/ProviderRuntimeRecoveryService.ts) · [Receipt race tests](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/apps/server/src/orchestration-v2/Orchestrator.receipt-race.test.ts)

The client subscribes separately to the environment shell and the thread it is viewing. Thread subscriptions can resume from a sequence, or receive a bounded snapshot when replay is stale or too large. Older timeline history can load separately. T3 Code prunes covered events, so its durable projections are essential to recovery from older history. A retained event’s schema must still decode during replay, even after the clients and server have upgraded. [Thread stream](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/apps/server/src/orchestration-v2/ThreadStream.ts) · [Snapshot and history endpoints](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/apps/server/src/orchestration-v2/http.ts) · [Projection maintenance](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/apps/server/src/orchestration-v2/ProjectionMaintenance.ts) · [Shared client state](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/packages/client-runtime/src/state)

Adapter tests use recorded native protocol transcripts to exercise the adapter, orchestrator, and projections together. This catches ordering and protocol details a fake often misses, such as stop arriving before native turn start, a child continuing after its caller settles, and model settings appearing only after a session is active. [Provider testing guidance](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/docs/internals/adding-a-provider.md#tests) · [Orchestration testing strategy](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/docs/orchestration-v2/testing-strategy.md)

# Cross-provider delegation

T3 Code exposes app-owned delegation through its Model Context Protocol (MCP) tools. The provider running the parent calls T3 Code. T3 Code selects and starts the child using the target provider’s normal adapter and returns the outcome through the parent’s thread. The two providers need a common T3 Code contract, so the parent can stay on Claude while a child runs on Codex, or vice versa. [MCP tool definitions](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/apps/server/src/mcp/toolkits/orchestrator/tools.ts#L75) · [MCP handlers](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/apps/server/src/mcp/toolkits/orchestrator/handlers.ts#L15) · [Delegation service](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/apps/server/src/mcp/OrchestratorMcpService.ts#L2679)

~~~mermaid
sequenceDiagram
    participant Parent as Parent agent on Claude
    participant MCP as T3 MCP
    participant O as T3 orchestrator
    participant Child as Child agent on Codex
    Parent->>MCP: Discover targets and delegate review
    MCP->>O: Validate caller and record task
    O->>Child: Start child thread and run
    MCP-->>Parent: Return task ID and status
    Child-->>O: Emit activity and result
    O-->>Parent: Offer a completion wake
    Parent->>MCP: Read result and acknowledge delivery
    MCP-->>Parent: Return result and child reference
~~~

## How a task starts

The provider session manager prepares an authenticated MCP binding for the app thread and provider instance. Codex receives it in per-thread app-server configuration. Claude receives the same service through its SDK MCP options. T3 Code also supplies orchestration instructions that tell the agent when to use its native subagent tool and when to call T3 Code for a different provider or checkout. Tool exposure, caller identity, and server-side authorization therefore have separate roles. [Session binding](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/apps/server/src/orchestration-v2/ProviderSessionManager.ts#L557) · [Codex MCP configuration](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/apps/server/src/orchestration-v2/Adapters/CodexAdapterV2.ts#L1509) · [Claude MCP configuration](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/apps/server/src/orchestration-v2/Adapters/ClaudeAdapterV2.ts#L2504) · [Agent instructions](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/packages/provider-core/src/server/orchestrationInstructions.ts#L4)

1. The parent calls `orchestrator_capabilities` to find enabled provider instances, models, available options, and runtime modes. The target shape accepts an instance ID, model, options, and, for a peer, environment and project IDs.
2. The parent calls `delegate_task` with a task and optionally a title, role, target, workspace isolation, and delivery mode. Roles include `review`, `implementation`, `research`, `design`, `test`, and `general`. A non-general role prefixes the task prompt. Authorization and workspace choices come from other fields and server policy.
3. For local calls the service resolves a thread-scoped caller and checks that the active parent run, root node, and provider instance match. It checks target availability and execution modes. An explicit broader permission or interaction mode is denied, and an unsupported inherited runtime mode can step down for the selected provider.
4. The service dispatches `delegated_task.request` with a command ID derived from caller scope and the request key. The orchestrator records an app-owned subagent and timeline entry under the parent, creates the child thread with lineage, and creates its initial run. Repeating a supplied request key reuses the command identity.
5. The child executes through the target adapter. Its initial user input is the delegated task, including any role prefix. The parent’s transcript is outside that initial prompt. The shared-checkout default gives the child access to the same working files through its own runtime permissions.

[Request and result contracts](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/packages/contracts/src/orchestratorMcp.ts#L99) · [Permission inheritance and task prompt](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/apps/server/src/mcp/delegatedTaskModes.ts) · [Delegation dispatch](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/apps/server/src/mcp/OrchestratorMcpService.ts#L2679) · [Task and child creation](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/apps/server/src/orchestration-v2/Orchestrator.ts#L9710)

A parent reviewing uncommitted changes can delegate to another provider with `role: "review"` and shared isolation. The task should identify the diff, relevant requirements, and the expected findings. A separate implementation can use `role: "implementation"` and `isolation: "worktree"`. For local delegation, worktree preparation starts from the parent checkout’s current Git commit. The child sees that commit, so any dirty files the task depends on need a separate delivery mechanism. The role is prompting, and worktree isolation is a checkout boundary. Read access, edit access, and human approvals still follow runtime policy. [Isolation contract](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/packages/contracts/src/orchestratorMcp.ts#L171) · [Worktree base selection](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/apps/server/src/mcp/OrchestratorMcpService.ts#L2742) · [Permission modes](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/docs/user/permission-modes.md)

## How the task settles

| Situation | What the parent can observe |
| --- | --- |
| Async start, the default | The call returns task and child identities plus status. Completion can wake the parent, so it can do other work or end its current turn. |
| Wait mode | The call waits up to 45 seconds for the result. A question, approval, or timeout can end the wait while the child remains live. The service can upgrade the wake policy when the wait exits without delivering a terminal result. |
| Child turn ends with descendants still running | Status distinguishes `working`, `waiting_for_children`, and `result_available`. The parent can tell an ended provider turn from a settled task. |
| Child has a pending request | The question reactor can wake the parent about a question. Task status surfaces pending questions and approvals. The runtime request retains its own authority checks. |
| Result available | The orchestrator can record result context transfer and offer a wake. Reading a final result through task status acknowledges its delivery. The result can include a child thread link and the reported model when available. |
| Cancellation | The task cancellation path stops its running and queued work, descendants, and task-related PR watches. A finished result remains available. |

[Wait and task result contract](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/packages/contracts/src/orchestratorMcp.ts#L171) · [Status computation and acknowledgement](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/apps/server/src/mcp/OrchestratorMcpService.ts#L1752) · [Wait implementation](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/apps/server/src/mcp/OrchestratorMcpService.ts#L1943) · [Question reactor](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/apps/server/src/orchestration-v2/DelegatedTaskQuestionReactor.ts) · [Cancellation tool](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/apps/server/src/mcp/toolkits/orchestrator/tools.ts#L118)

Completion is a delivery operation with durable state. Offering a wake and acknowledging the result are distinct milestones. The parent can read a result while a wake is queued, can have several children finish close together, or can restart before consuming a wake. The orchestrator groups compatible completion notices, reconciles queued delivery with acknowledgements, and recovers outstanding work. This allows a parent that has already ended its original turn to receive a child result later. [Completion planner](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/apps/server/src/orchestration-v2/Orchestrator.ts#L14421) · [Completion delivery tests](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/apps/server/src/orchestration-v2/DelegatedCompletionDelivery.test.ts)

## Native subagents and remote children

A provider-native child comes from the provider’s own runtime. The adapter projects the child lifecycle and, when exposed, its native IDs and conversation. An app-owned child comes from T3 Code’s delegation command and has the app’s task and lineage records. This is the boundary that allows choosing a different provider. Both appear in the shared execution model, with an explicit `origin` field and different available controls. [Subagent schema](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/packages/contracts/src/orchestrationV2.ts#L931) · [Provider event contract](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/packages/provider-core/src/server/ProviderAdapter.ts#L113) · [Native subagent projection](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/packages/provider-core/src/server/subagentProjection.ts)

A linked-environment target runs as an ordinary thread on the remote host. The parent host retains its task record and follows the remote thread using the peer connection. The task result can point to a remote environment and thread, and the local parent can be notified when remote work ends. That remote host owns its checkout and provider credentials. Outside MCP clients have a separate access model. The task tools act as the calling thread and depend on thread-scoped authority. [Remote delegation](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/apps/server/src/peer/RemoteDelegation.ts#L86) · [MCP tool access](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/apps/server/src/mcp/McpToolAccess.ts) · [Outside agents](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/docs/user/outside-agents.md)

For Devcroft, a review or implementation task can be treated as an ordinary child thread with a structured assignment, workspace, permission ceiling, and result. A durable task ID and parent notification solve the coordination problem even when the parent turn ends before its reviewer. See [the proposed Devcroft design](05-devcroft-architecture.md#switching-and-delegation).

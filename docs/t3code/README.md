# T3 Code architecture for Devcroft

T3 Code gives users one conversation interface for several coding agents. Its server owns the conversation, execution state, and workspace operations. Provider adapters translate that shared model into the structured protocols of individual agents. That split lets a user change providers inside a conversation and lets an agent delegate part of its work to a different provider.

This study is based on the local T3 Code checkout at `106a1e03b73f4a0d114cd7f7de20be2c343c22c1` (November 19, 2026). Source links pin that revision. The pages follow the shipped orchestration V2 code. T3 Code’s design notes have useful vocabulary, but some behavior described there, such as model-generated handoff summaries, differs from the current implementation. [Source tree](https://github.com/pingdotgg/t3code/tree/106a1e03b73f4a0d114cd7f7de20be2c343c22c1) · [Architecture overview](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/docs/internals/overview.md) · [Original switching design](https://github.com/pingdotgg/t3code/blob/106a1e03b73f4a0d114cd7f7de20be2c343c22c1/docs/orchestration-v2/provider-switching-and-context.md)

| Page | What it explains |
| --- | --- |
| [Unified agent runtime](01-unified-agent-runtime.md) | Provider drivers and instances, protocol adapters, conversation and execution identity, capabilities, persistence, and streaming |
| [Provider switching and context handoffs](02-provider-switching.md) | How one thread moves between providers, how a returning provider resumes, and how handoffs handle context limits and failures |
| [Cross-provider delegation](03-cross-provider-delegation.md) | How an agent selects another provider, starts a child, supervises it, and receives its result |
| [Workspace and platform features](04-workspace-and-platform.md) | Worktrees, checkpoints, forks, steering, scheduling, PR tracking, remote environments, and migration |
| [Devcroft architecture implications](05-devcroft-architecture.md) | The current Devcroft boundary, the proposed agent service and data model, migration options, and decisions to resolve |

Start with [the Devcroft architecture](05-devcroft-architecture.md) if you are deciding what to carry forward. The other pages document the source behavior behind those decisions.

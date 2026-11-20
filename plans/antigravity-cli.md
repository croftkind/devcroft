# Support Google Antigravity CLI

## Goal

Use the installed `agy` CLI from Devcroft's Agent workspace and reopen local Antigravity conversations from the checkout's history.

## Non-goals

Install or authenticate Antigravity, add an ACP interface, or infer live activity from the Antigravity terminal display.

## Files that own this behavior

- `src/agent.rs` and `src/agent_icons.rs` register the harness and its icon.
- `src/agent_activity/` launches the terminal command, including interactive guide prompts.
- `src/agent_sessions/` reads local conversation summaries and builds resume arguments.
- `src/agent_accounts.rs` enforces the space's account policy.
- `src/markdown_references.rs` accepts portable links to saved Antigravity conversations.
- `docs/terminal.md` describes availability and how to select the harness.

## Approach

Add Antigravity to the common harness registry. Use `agy --prompt-interactive` for guide launches and `agy --conversation` with the native ID for resumes. Read the installed CLI's SQLite summary index in read-only mode and scope conversations to their primary local workspace. Require a shared-account space until the interactive CLI provides a verified way to isolate credentials and session storage.

## Acceptance criteria

- Settings, onboarding, and New session offer Antigravity through the common harness registry.
- Fresh and guide-driven launches stay in the interactive CLI in the checkout.
- Saved local conversations appear with their title, update time, and checkout, and reopen by ID. Child conversations and conversations without a local workspace stay out of the catalog.
- Portable Markdown references can link to Antigravity conversations with the `agy` provider ID.
- An isolated space cannot launch Antigravity with shared credentials or access its shared history.

## Verification commands

- `cargo fmt --all -- --check`
- `cargo test --locked`
- `cargo build --locked`
- `git diff --check`
- Open a fresh Antigravity session and resume an existing one in a separate Devcroft desktop instance.

## Verification results

The build, formatting check, and diff check passed. The full test suite passed with 889 tests and seven ignored. Independent review covered the launch flags, local history, account policy, onboarding, and Markdown links. [Review](devcroft:artifact/art-thhujvv4)

Using the installed Antigravity CLI 1.5.13, the debug desktop displayed Antigravity and its icon in Settings and New session, opened a fresh terminal session to its workspace trust prompt, and resumed an existing conversation from its session card to an interactive prompt. During the resume check, Antigravity reported HTTP 500 from its account eligibility check. No prompt was submitted.

## Risks / open questions

- The local summary index is an implementation detail verified against Antigravity CLI 1.5.13. Schema changes may affect history discovery.
- Activity badges need a separate provider-specific adapter.

# Background agent completion indicator

## Goal

Show the topbar completion count for confirmed work completed in other repository workspaces. Visiting the repository acknowledges its notifications, regardless of the selected tab.

## Non-goals

- Change provider terminal layout classifiers or the attention indicator.
- Change per-session Finished status, which remains tied to viewing the agent result.

## Files that own this behavior

- `src/agent_activity/mod.rs`: completion lifecycle, foreground checkout, notifications.
- `src/workspace.rs`: foreground workspace synchronization, topbar counts, regression coverage.

## Approach

- [x] Track the foreground checkout independently of the visible agent launch.
- [x] Suppress foreground completion notifications and acknowledge them on repository entry.
- [x] Clear stale completions on new work, requests, failure, interruption, and exit.
- [x] Verify foreground/background navigation, multiple sessions, idle redraws, and later turns.
- [x] Obtain an independent review and record it as a Devcroft artifact.

## Acceptance criteria

- Starting or idle agents without a completed turn never create a notification.
- Confirmed background work produces one notification per agent; idle redraws do not re-notify.
- The current repository contributes no completion notifications on any workspace tab.
- Visiting a completed repository clears its notifications; leaving does not resurrect them.
- Starting another turn or interrupting/failing/exiting clears a prior completion.
- Other repositories and active-space filtering continue to work.

## Verification commands

- `cargo fmt --all -- --check`
- `cargo test --locked agent_activity:: -- --test-threads=1`
- `cargo test --locked workspace::tests::completion_indicator -- --test-threads=1`
- `cargo clippy --locked --all-targets -- -D warnings`
- Focused computer-use inspection of the topbar when feasible without disturbing active sessions.

## Risks / open questions

- Foreground checkout must use the same canonical identity as managed launches.
- Per-session result visibility and repository notification acknowledgement must stay separate.
- Preserve existing Projects acknowledgement and active-space behavior.

## Verification results

- 48 agent activity, provider, and terminal tests passed. The existing HTTP fixture required localhost socket access outside the sandbox.
- 3 workspace completion indicator tests passed, covering Projects acknowledgement, active spaces, and repository navigation across all tabs.
- Formatting, whitespace checks, and Clippy for all targets passed.
- Computer-use inspection confirmed the original green finished badge in the current repository while sessions showed Working and Idle. The running app was left intact to preserve active sessions; post-change behavior is covered by the automated workspace tests.
- Independent review found and verified the fix for a dismissed input prompt falsely finishing a turn. No actionable findings remain. Devcroft review: [Background agent completion review](devcroft:artifact/art-73nwtt82).

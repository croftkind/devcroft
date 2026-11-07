# Global session sidebar and persistent pins

## Goal

Make the Agent sidebar useful across projects: show open sessions across the current window and pinned conversations across repositories, respecting the selected space. Keep recent unpinned, closed conversations scoped to the current checkout.

Refine cards into repository/time, logo/title, and harness/status rows. Reveal pin and close controls together in the top-right on hover, and include the Pin glyph in the app's embedded assets.

Keep the actions revealed while hovering their buttons, use two-row Recent cards without redundant repository labels, and distinguish sections with full-width header backgrounds and spacing. Show repository scope on header hover.

Make the three sections independently collapsible with gpui-kit's accordion primitives. Keep counts and scope tooltips visible, start expanded, and remember expansion within the current window across repository/space changes. Navigation only visits expanded rows.

## Non-goals

- Discover agent processes outside this Devcroft window.
- Add summaries, session previews, task groups, or automatic attention-based ordering.
- Sync pins between devices; session identities and provider stores are machine-local.

## Files that own this behavior

- `src/workspace_sessions.rs`: section projection, card actions, repository switching, keyboard navigation.
- `src/workspace.rs`: workspace state and initialization.
- `src/data/device.rs`: atomic device-local pin persistence.
- `src/pane.rs`: shared unstarted initialization for process-free workspace regression fixtures.
- `src/app_assets.rs`: embedded Pin icon availability.
- `docs/keyboard-reference.md` and `README.md`: session sidebar behavior.

## Approach

- [x] Project Open, Pinned, and Recent sections from one shared model used by rendering and keyboard navigation.
- [x] Include every open session in the selected space, with title, harness logo/name, status, and repository; reuse exact-session activation to restore its workspace.
- [x] Persist pins using the full provider/store/session identity. Resolve their metadata from the existing account-filtered catalog, independently of recent limits.
- [x] Offer pin/unpin on catalog-backed cards. Open pinned sessions appear once in Open and return to Pinned when closed. Recent excludes open and pinned identities before applying its limit.
- [x] Follow existing space rules: registered repositories match the selected space; explicitly open unregistered checkouts remain available.
- [x] Verify space filtering, identity, persistence, duplicate suppression, navigation order, and cross-repository activation; obtain agent review and focused GUI verification.
- [x] Refine card hierarchy, show time for open sessions, restore the Pin asset, and verify hover controls visually.
- [x] Fix action hover occlusion, compact Recent cards, and strengthen section boundaries; verify the final hover path and review the changes.
- [x] Move scope captions into header tooltips, extend header backgrounds across the sidebar, and remove section divider lines.
- [x] Add independent accordion sections with accessible keyboard/pointer triggers, retained window state, and navigation matching expanded content; test, review, and verify visually.

## Acceptance criteria

- Open sessions from multiple repositories appear together and switch to the exact retained session without relaunching it.
- Pinned conversations survive app restart and remain reachable outside the repository's recent limit.
- Open and Pinned sections immediately follow named-space and All-spaces selection.
- Recent stays local to the current repository, with no duplicate open or pinned cards.
- Pin/unpin writes atomically, preserves unrelated device settings, and reports failures without changing the visible pin state.
- Keyboard navigation visits precisely the rendered session rows in section order.
- Open and Pinned cards show repository and time above the logo/title, with harness and status below. Recent cards omit the repository row and show time beside the title, with harness below.
- Pin/close controls share the top-right row on hover without moving card content; moving onto Pin or Close keeps the timestamp hidden and the controls visible.
- Full-width section headers separate the groups without extra divider lines. Their hover tooltips distinguish cross-repository sessions from current-repository history.
- Keyboard focus reveals the same controls. Activating a pin through accessibility updates the pin without opening or switching sessions.
- Each header toggles its own section, with a disclosure icon, expanded accessibility state, and keyboard activation. Other sections and the active session remain unchanged. All start expanded; current-window expansion survives repository/space switches.
- Collapsed sections retain their counts and scope tooltips, hide their cards and empty/error states, and are skipped by session navigation.

## Verification commands

- `cargo fmt --all -- --check`
- `cargo check --locked`
- `cargo clippy --locked --all-targets -- -D warnings`
- `cargo test --locked`
- `cargo build --locked` followed by focused GUI verification through the computer-use agent.

## Risks / open questions

- A deleted or currently unavailable provider session stays pinned on disk but cannot appear until the account-filtered catalog discovers it again.
- Repository recent history remains visible when its workspace stays open after changing spaces, matching existing behavior; global sections follow the selected space.
- Opening historical conversations remains subject to the existing provider/account validation and checkout availability checks.

## Verification outcome

- Formatting, `cargo check --locked`, clippy across all targets with warnings denied, and the final build pass.
- Full test suite: 867 unit tests and 8 CLI integration tests pass; 7 existing tests are ignored. The suite requires permission to bind a temporary localhost server for an existing OpenCode provider test.
- Added regressions cover space-filtered section order, exact provider/store/session identities, pin persistence across store reopen, failure-safe writes, recent limits, discovery recovery, and exact retained-pane activation. Live activation now invalidates older asynchronous resumes so the latest selection wins.
- Native GUI verification confirmed cards from multiple repositories, exact session/workspace switching, pinning and unpinning, persistence across app processes, open-pin duplicate suppression, return to Pinned on close, and readable cards at the existing 280px sidebar width.
- Named-space filtering and navigation-key delivery could not be visually verified because the automation could not reliably focus the separate test window or read its accessibility elements. These behaviors have automated coverage; no product freeze was observed. Test pins and test processes were cleaned up; the original app window was left open.
- Review artifact: [Session sidebar review](devcroft:artifact/art-dhsynarx). No outstanding findings.

## Card refinement verification

- Included the missing Pin SVG in the embedded icon catalog and extended the existing asset-availability regression.
- Open and Pinned cards use repository/time, logo/title, and harness/status rows. Recent uses compact logo/title/time and harness rows. Open cards use catalog activity time, with opening time as fallback.
- Hover, keyboard focus, and the navigation cursor reveal horizontal pin/close actions in the timestamp's reserved space. Faded actions retain their accessibility hitboxes and stop propagation to the card underneath.
- The Pin asset test and all 31 workspace tests pass, including a new regression that pins/unpins without prior hover and checks that session navigation and repository selection remain unchanged. Formatting, compile/build, clippy across all targets with warnings denied, and diff checks pass.
- Native screenshots confirmed the hierarchy, real pushpin glyph, aligned close button, no layout shift, and clean long-title truncation. Physical clicks and accessibility Pin/Unpin on Recent both work without opening the conversation; the existing user pin was preserved.
- Focused reviewer follow-up found no outstanding issues; the review artifact above includes this refinement.

## Section and hover follow-up

- Reproduced the hover defect in the previous build: moving from a pinned card to its pin button made the pin disappear and the timestamp return beneath the tooltip.
- Removed the actions container's mouse occlusion so that hovering either the card or its controls preserves the card hover group. Actions keep their accessibility hitboxes and stop click propagation.
- Recent cards omit the redundant repository row. Each section has a stronger header, scoped caption, count badge, and boundary spacing/divider when another section precedes it.
- All 31 workspace tests pass. The interaction regression now checks the card-to-pin hover path and timestamp restoration for Recent and Pinned, alongside pin/unpin persistence without session activation. Compile/build, formatting, clippy across all targets with warnings denied, and diff checks pass.
- Native GUI verification confirmed body → Pin → tooltip keeps the pin visible and timestamp hidden, and moving away restores the timestamp. Recent is compact without repository labels, long titles truncate cleanly, and all sections have clear boundaries and scope captions. Accessible Pin/Unpin still preserves the active session. Physical pin/unpin and Open close were not repeated in this build. The existing pin and original app remained intact; test pin and process were cleaned up.
- Reviewer follow-up found no outstanding issues and updated the existing review artifact.

## Header polish verification

- Removed the extra scope-caption rows and moved those details into header hover tooltips. Open/Pinned describe repositories in the selected space; Recent describes the current repository.
- Header backgrounds now span the sidebar with square corners. Cards, empty/error states, and footer retain their inset. Removed the separate lines between sections.
- Build, clippy across all targets with warnings denied, formatting, diff checks, and the pin-hover/activation regression pass.
- Native GUI verification confirmed edge-to-edge headers, no captions or extra section divider lines, intact card/footer insets, clean truncation, and correct Open/Recent scope tooltips. No pins changed; the test process was closed and the original app remained intact.

## Collapsible sections verification

- Used gpui-kit's base accordion root, item, header, trigger, and panel to retain the full-width header styling. Headers add chevrons, keyboard focus, expanded accessibility state, scope descriptions/tooltips, and native Enter/Space activation. Sections expand independently and start open.
- Expansion and focus handles belong to the workspace window, so repository/space changes retain each section's state. Closed panels unmount their cards and empty/error states; counts still come from the complete space-filtered projection. Session navigation applies the same expansion mask as rendering.
- All 32 workspace tests pass, including collapse/reopen of each section, hidden-row navigation, header focus/native keyboard activation, stale cursor handling, intentional j/k followed by retained-session activation, pin interaction, and cross-repository expansion retention.
- Review identified and verified the correction for header focus winning after intentional j/k movement: item navigation now focuses the sidebar so Enter opens the highlighted session. No outstanding findings remain in the existing review artifact.
- Final build, formatting, clippy across all targets with warnings denied, and diff checks pass. Native GUI verification confirmed independent/all-section collapse and reopening, preserved counts/chevrons/footer/current session, and retained expansion after a repository roundtrip. Scope text remains available in accessibility help. Visual tooltip and native keyboard input could not be checked reliably because foreground/window placement was unreliable; keyboard activation has automated coverage. Test process was closed, existing pins preserved, and the original app remained open.

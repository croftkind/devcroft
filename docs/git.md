# Git workspace page

Git is a full workspace page for the current checkout. Install `git` on
the application's PATH. Open the branch/status menu in the workspace header and choose **Open Git**, use `Cmd+J`,
then `g` on macOS (`Ctrl+J`, then `g` on Linux/Windows), or choose **Open Git**
in the actions palette. Dialogs, including branch creation, open above the
page. The **back arrow** returns to the previous view and focus without changing the selected workspace tab.

**Changes** and **History** use the regular tab bar on the right side of the
workspace header. A split button after the tabs runs **Fetch**; its dropdown
offers pull, push, and refresh actions.

The same header branch picker stays available in workspace views and on the Git page. It searches local branch names and commit summaries,
switches branches, and offers **New branch**. The current matching branch is
pinned first. Hover the picker to see the full checkout path and upstream.

## Keyboard and refresh

Use **Tab / Shift+Tab** to move through modes, lists, inputs, and action
buttons. **Enter / Space** activates a focused button, including a file or
branch. In the history commit list, **Up / Down**, **Home / End**, and
**Enter / Space** select a commit. In a focused diff, **Up / Down** scroll a
line, **Page Up / Page Down** scroll a page, and **Home / End** reach its ends.
The history changed-file list uses the same selection keys. Commit and merge editors keep their normal typing, clipboard, search, and
undo/redo shortcuts. Escape first dismisses an editor's search when open;
otherwise **Escape** dismisses the active popover or dialog. Use **Back** (or
`Cmd/Ctrl+J`, then `b`) to return to the previous view; navigation mode can also
open another workspace tab or Home. Commit messages and merge Results survive
leaving and returning, including checkout switches; quitting clears commit
messages. Wait for an active Git operation to finish before switching pages or checkouts.

Repository changes from agents, the terminal, or other tools refresh
automatically. **Refresh repository** explicitly reloads status and history.
The header refreshes from the same repository snapshot as the Git page. Operations run in the
background, one mutation at a time per checkout; progress and failures appear
on the page. Switching or creating a branch from another workspace view briefly shows
progress, then returns to that view and focus. Failures also appear as notifications. A refresh can reset history selection when refs change.

## Changes and commits

**Changes** groups **Merge changes**, **Staged**, **Unstaged**, and **Untracked**
files in a sidebar. Conflict and untracked sections appear when they contain files.
Rows show the filename above its directory (or original path for renames), with
compact, labeled-on-hover stage, unstage, discard, and trash controls. Select a file to see HEAD-to-index,
index-to-worktree, or empty-to-worktree content as appropriate. Renames show
both paths. **Show more** exposes further files in large sections.

Non-conflict Changes and History diffs support **Unified** and **Split**
layouts, **Wrap** for long lines, and **Previous change / Next change**. These
controls share a single compact file header in Changes, alongside the path,
line counts, and editor button. Hover the path for the full comparison details.
The up/down arrows jump between changes.
Display choices remain through refreshes and when selecting another file. Drag over source text or select lines from the gutter,
then use `Cmd+C` on macOS (`Ctrl+C` elsewhere) to copy source without diff
markers or line numbers. `Shift+Up` / `Shift+Down` extends a line selection.
Only supplied patch context is available. Files beyond the existing 4 MiB
read limit show an explanation; diffs beyond the 4,000 retained hunk-line limit
show a persistent truncation notice. Omitted source cannot be expanded or copied.

**Stage / Unstage** affect one file; **Stage All / Unstage All** affect all
eligible changes from the **Unstaged / Staged** section headers. **Stage all**
also includes untracked files. Bulk staging is blocked while conflicts remain. Enter a
message in the **Commit** dialog, opened from the header action group, and choose
**Commit changes** after staging. The dialog shows the branch and staged files;
canceling or a failed commit preserves the message. Blank messages, an empty
index, and unresolved conflicts disable commit. Git runs configured hooks
and signing; a failure keeps the message, and success clears it.

**Discard** requires a path-specific confirmation and restores tracked files
from the index, preserving staged content. On macOS, **Trash** removes
untracked files recoverably; recover them through macOS Trash. Other platforms
omit untracked removal and rename discard where recoverable trash is unavailable.
Actions that overwrite a file with an unsaved built-in editor draft are
blocked. **Open in Editor** switches to the Editor tab and opens the working-tree file in
Devcroft's built-in editor.

Binary, oversized, invalid UTF-8, symlink, submodule, missing, and type-changed
content receives an explicit explanation instead of a text diff. Diff reads
are bounded; truncated content is labeled.

## Branches and remotes

The header branch picker searches local branch names and commit summaries.
Select a branch to switch. **New branch** opens a dialog to name a branch starting at the current HEAD,
then creates and switches. Use History's **Create branch here** for a specific commit. Git rejects switches that would overwrite work.

The header's **Fetch** button and **Git actions** dropdown offer remote operations.
**Pull (fast-forward only)** uses `--ff-only`: divergence fails without an implicit
merge commit. **Pull with rebase…** explicitly replays commits; it requires a clean worktree and confirms source and target. No automatic
stash occurs. Push never forces; without an upstream, choose a remote and
remote branch to set one on the first push.

Git retains repository/global configuration, credential helpers, SSH-agent
environment, hooks, and signing setup. Devcroft does not collect or store
credentials. Configure authentication and signing to complete without terminal
input; when interaction is required, finish it in a terminal and retry.
Failures show bounded command output and preserve the repository operation.

## History

**History** loads at most 100 commits per page; **Load more** adds another
page. Filtering searches only loaded metadata. Choose HEAD or a branch scope.
Rows show graph lanes, commit id, subject, author, time, and ref decorations.
Select a commit for its summary and body; **Details** expands the full commit id,
author, committer, and parents. Changed files sit beside the diff. Select a file for its
immutable parent-to-commit diff. Root commits compare to an empty tree;
merge commits explicitly compare to the first parent. Oversized metadata is
truncated with a notice.

History uses the same diff display and source-copy controls as Changes.

**Copy commit id**, **Create branch here**, and confirmed **Check out commit**
are available. Checking out a commit enters detached HEAD. Create or switch
to a branch before committing work you intend to keep.

## Conflicts and recovery

Merge, rebase, cherry-pick, and revert state comes from Git, including linked
worktrees. Conflicts appear in **Merge Changes**. Text conflicts open
**Current** and **Incoming** above an editable **Result**, with optional
**Base**. Labels identify the actual operation; during rebase Git's Current
side is the rebased destination and Incoming is the replayed commit.

Existing manual worktree edits initialize Result. **Previous / Next** choose
conflict blocks. **Accept Current**, **Accept Incoming**, and **Accept Both**
edit Result only. **Compare with Base** reveals the ancestor. **Generate
diff3** regenerates Result and can be undone. Result supports typing,
syntax highlighting, search, and undo/redo.

**Save Result** atomically writes without staging. **Mark Resolved** writes
and stages that one path, rechecking disk and index state and preserving line
endings and executable mode. External changes block overwriting. Marker-like
lines show a warning; their absence does not prove the merge is correct.
Binary and structural conflicts offer applicable whole-file **Keep Current**,
**Keep Incoming**, **Delete**, or external-editor actions. Unsafe directory
replacement, submodule worktree destruction, and uninspectable content are
refused and need external handling.

Unsaved Results survive file/mode changes and are journaled under the normal
Devcroft data root, independently of the repository. Reopening or restarting
offers recovery; stale or externally resolved drafts remain available to copy
or discard. A warning and recovery-folder action expose corrupt, oversized,
or unreadable journals. Text results are capped at 2 MiB; worktree inspection
at 64 MiB. Recovery discovery is bounded to 256 files and 32 MiB total.

The operation banner offers **Continue**, confirmed **Abort**, and for rebase
confirmed **Skip**. Continue is disabled until Git has no unmerged entries.
Git executes its usual commands, hooks, and configuration; failures retain
operation state and show output. Abort and Skip recheck captured state before
running. Git owns operation state across app restarts. Review the result and
save or copy any drafts you want to keep before abandoning an operation.

Interactive rebase editing, stash management, force push, submodule management,
and line/hunk staging remain terminal workflows.

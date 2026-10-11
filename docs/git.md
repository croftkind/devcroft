# Native Git dialog

Git runs inside a near-window-sized native dialog for the current checkout.
Install `git` on the application's PATH. Open the dialog with `Cmd+J`, then
`g` on macOS (`Ctrl+J`, then `g` on Linux/Windows), or **Open Git** in the
actions palette. Git is separate from the workspace tabs; the branch header
is display-only.

## Keyboard and refresh

Use **Tab / Shift+Tab** to move through modes, lists, inputs, and action
buttons. **Enter / Space** activates a focused button, including a file or
branch. In the history commit list, **Up / Down**, **Home / End**, and
**Enter / Space** select a commit. In a focused diff, **Up / Down** scroll a
line, **Page Up / Page Down** scroll a page, and **Home / End** reach its ends.
The history changed-file list uses the same selection keys. Commit and merge editors keep their normal typing, clipboard, search, and
undo/redo shortcuts. Escape first dismisses an editor's search when open;
otherwise **Escape** or the close button closes the dialog and restores
workspace focus. Closing waits for an active Git operation to finish. Commit messages survive closing and reopening in the same
workspace, including checkout switches; quitting the app clears them.

Repository changes from agents, the terminal, or other tools refresh
automatically. **Refresh repository** explicitly reloads status and history.
The header uses the dialog's snapshot while open. Operations run in the
background, one mutation at a time per checkout; progress and failures appear
in the dialog. A refresh can reset history selection when refs change.

## Changes and commits

**Changes** groups **Merge Changes**, **Staged Changes**, **Changes**
(unstaged), and **Untracked Files**. Select a file to see HEAD-to-index,
index-to-worktree, or empty-to-worktree content as appropriate. Renames show
both paths. **Show more** exposes further files in large sections.

**Stage / Unstage** affect one file; **Stage All / Unstage All** affect all
eligible changes. Bulk staging is blocked while conflicts remain. Enter a
commit message and choose **Commit** after staging. Blank messages, an empty
index, and unresolved conflicts disable commit. Git runs configured hooks
and signing; a failure keeps the message, and success clears it.

**Discard** requires a path-specific confirmation and restores tracked files
from the index, preserving staged content. On macOS, **Trash** removes
untracked files recoverably; recover them through macOS Trash. Other platforms
omit untracked removal and rename discard where recoverable trash is unavailable.
Actions that overwrite a file with an unsaved built-in editor draft are
blocked. **Open in Editor** closes Git and opens the working-tree file in
Devcroft's built-in editor.

Binary, oversized, invalid UTF-8, symlink, submodule, missing, and type-changed
content receives an explicit explanation instead of a text diff. Diff reads
are bounded; truncated content is labeled.

## Branches and remotes

**Branches** searches local and remote names and commit summaries. Select a
branch to inspect its commit and upstream. **Switch** checks out a local
branch; a remote branch can be adopted as a new local tracking branch.
**Create Branch** validates a name and offers HEAD or a selected start point,
then creates and switches. Git rejects switches that would overwrite work.

The dialog toolbar offers **Fetch**, **Pull**, **Pull with Rebase**, and
**Push**. Pull always uses `--ff-only`: divergence fails without an implicit
merge commit. Pull with Rebase and **Rebase onto** explicitly replay commits;
they require a clean worktree and confirm source and target. No automatic
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
Select a commit for parents, body, and changed files, then a file for its
immutable parent-to-commit diff. Root commits compare to an empty tree;
merge commits explicitly compare to the first parent. Oversized metadata is
truncated with a notice.

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

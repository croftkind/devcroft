# Review comments

Review uses the same diff component as Git Changes and History. Choose
**Unified** or **Split**, and use **Previous change / Next change** to navigate.
Split view always wraps long lines within each column. Unified view scrolls
horizontally. Select a file in the Files sidebar to jump to its diff. Drag over
source text, double-click a word, or select lines from the gutter, then use
`Cmd+C` on macOS (`Ctrl+C` elsewhere) to copy source without diff markers.
`Shift+Up` / `Shift+Down` extends a gutter selection.

Hover a source line and click its **+** button to write feedback. To comment on
an inclusive range, select lines in the gutter and click **+** on one of those
lines. Ranges stay within one file and one side; Split view makes the old and
new sides explicit. The editor appears below the range's final line, receives
keyboard focus, and offers **Save comment**. Use the file header's
**Open in Editor** action to open its current working-tree file. The header of
the file at the top of the diff stays pinned there, spans the pane rather than
the code width, and keeps **Open in Editor** and the viewed toggle reachable
when Unified lines are wider than the pane.

Saved comments stay visible as threads below their anchored ranges. Each thread
provides **Edit**, **Resolve / Reopen**, and **Delete**, with editing inside the
thread. The sidebar is an index showing file locations and comment text only;
it has no code excerpts or per-comment controls. The toolbar button before Refresh shows the comment count and toggles this sidebar. Sidebar cards retain subtle
green backgrounds for resolved comments and yellow backgrounds for outdated
comments, including when resolved; inline threads in the main pane have no status tint.
Clicking a comment card opens its file, scrolls to its inline thread, and highlights the anchored range.
Edit reveals and focuses the editor. Resolve/Reopen and Delete act independently
without jumping. Outdated or hidden anchors appear
under the file header; comments whose files are absent from the diff appear in
the main pane's "Comments outside the current diff" section. Outdated threads
retain their saved code excerpts. All controls remain available there, even when
the diff is empty. Delete permanently removes that
comment, including its body and anchor; resolve retains it for later review.

The open Review tab checks for code changes automatically. Marking a file
**Viewed** collapses it; if either side changes afterward, its viewed mark is
cleared and the file expands. Unchanged files keep their review progress,
including across new commits on the same branch. Manual collapse controls
remain independent of viewed marks. Automatic checks update the UI only when
the displayed review changes. Unchanged refreshes preserve the diff selection
and viewport. When source
changes, Review restores the selected source range, following unchanged lines
when they move, or returns to the selected file. The Files sidebar remains a
direct navigation control; scrolling the diff does not change its selection.
Returning to Review and automatic portable sync preserve an unchanged viewport.

Use **Refresh**, or return to the Review tab, after the agent changes
comments. Unsaved drafts survive same-pair refreshes. Switching branch pairs or
review scopes clears the editor. Open drafts follow unchanged code when lines
move; drafts on removed code remain available with an outdated label. Automatic
anchor relocation also preserves edits to saved comments. An edit based on an
externally modified comment fails with a refresh message instead of overwriting
it; after refreshing, click
Edit again to use its current revision.

## Agent workflow

These commands run without the desktop. They use the current checkout by default;
`--checkout PATH`, `--base BRANCH`, and `--remote NAME` select another review.
The base defaults to the same suggestion used by the desktop, and the remote
defaults to `origin`. Add `--uncommitted` for comments made in that scope.

```sh
devcroft review list --open
devcroft review resolve c1 --revision 1
devcroft review list --resolved
devcroft review delete c1 --revision 2
```

`list` always emits a JSON array. Each element contains `comment` (including
`id`, `body`, `resolved`, `revision`, and `anchor`) and `excerpt`. Anchors include
the repository-relative path, `old` or `new` side, inclusive one-based `start`
and `end`, and an independent `outdated` flag. Full saved file contents are not
included in CLI output. `list --resolved` lists resolved comments; omitting
filters lists all comments. `reopen ID` returns a comment to open status.

Ask the agent to list open comments, address the feedback, run appropriate
checks, and resolve the addressed IDs. The optional `--revision` rejects an
operation if the comment changed after the agent read it. Prefer using it in
agent workflows. Successful mutations exit zero; missing IDs, stale revisions,
and store failures exit nonzero. The CLI cannot create comments or edit bodies.
Deleting resolved comments is a separate, explicit operation on their IDs.

## Persistence and changed code

Comments live in `devcroft-review/comments.json` under Git's common directory
(normally `.git/devcroft-review/comments.json`). Linked worktrees share that
store. They are local to the repository and are not committed, exported, or
synced through Devcroft's portable data. Removing the repository's Git directory
also removes this feedback.

The identity is remote name + base branch name + head branch name, with separate
full-diff and uncommitted scopes. New commits preserve that identity. Detached
HEAD uses the commit ID in place of the branch name. Renaming a branch creates
a different pair. No branch-pair history or cross-machine portability is needed.

Each anchor retains its source text so relocation can compare complete files,
including unchanged lines outside visible hunks. Insertions/deletions before an
unchanged range move its line numbers. Edited, removed, unavailable, or ambiguous
ranges become **outdated** and keep their original excerpt and feedback.
Restoring the original text can place them again. Repeated lines require unique
surrounding context; the store favors an outdated comment over an uncertain
placement. Exact-content renames detected by the diff follow the new path.
Old-side anchors are checked against the current base snapshot, independently
of changes on the new side.

Writes use a cross-process lock and atomic file replacement. Desktop updates
check revisions, and malformed store files produce errors instead of being
replaced with an empty store. Source snapshots are local anchoring data and may
contain the same sensitive text as the reviewed files.

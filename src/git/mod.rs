mod branches;
mod conflicts;
mod diff;
mod history;
mod merge_editor;
mod model;
mod operations;
mod remote;
mod repository;
mod view;
mod watch;

pub(crate) use view::{
    BranchPickerAction, BranchPickerRequested, CommitDraftChanged, GitView, OpenGitFile,
    SnapshotChanged,
};

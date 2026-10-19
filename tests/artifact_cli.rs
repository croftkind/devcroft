use serde_json::Value;
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};
fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_devcroft"))
        .args(args)
        .env("DEVCROFT_DATA_DIR", root)
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .output()
        .unwrap()
}
fn json(root: &Path, args: &[&str]) -> Value {
    let output = run(root, args);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
#[test]
fn resource_and_feedback_workflow_without_desktop() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("data");
    for key in ["repo", "other"] {
        let path = root.join("portable/repositories").join(key);
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("repository.json"), "{}").unwrap();
    }
    let markdown = temp.path().join("plan.md");
    fs::write(&markdown, "# Plan\n- [ ] Implement\n").unwrap();
    let sessions = temp.path().join("sessions.json");
    fs::write(&sessions, r#"[{"repository":"other","title":"Discussion","key":{"provider":"codex","store":"/tmp/provider","id":"session-id"}}]"#).unwrap();
    let created = json(
        &root,
        &[
            "artifact",
            "create",
            "--repository",
            "repo",
            "--title",
            "Plan",
            "--kind",
            "plan",
            "--content-file",
            markdown.to_str().unwrap(),
            "--sessions-file",
            sessions.to_str().unwrap(),
            "--json",
        ],
    );
    let id = created["artifact"]["id"].as_str().unwrap();
    let revision = created["revision"].as_str().unwrap();
    assert_eq!(created["artifact"]["sessions"][0]["repository"], "other");
    assert!(
        root.join("portable/artifacts")
            .join(id)
            .join("artifact.md")
            .is_file()
    );
    assert!(!root.join("portable/tasks").exists());
    let list = json(
        &root,
        &["artifact", "list", "--repository", "repo", "--json"],
    );
    assert_eq!(list["artifacts"].as_array().unwrap().len(), 1);
    let empty = json(
        &root,
        &["artifact", "list", "--repository", "other", "--json"],
    );
    assert!(empty["artifacts"].as_array().unwrap().is_empty());
    let added = json(
        &root,
        &[
            "artifact",
            "comment",
            "create",
            id,
            "--revision",
            revision,
            "--body",
            "Clarify scope",
            "--json",
        ],
    );
    let comment = added["artifact"]["comments"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let listed = json(&root, &["artifact", "comment", "list", id, "--json"]);
    assert_eq!(listed["comments"][0]["body"], "Clarify scope");
    let stale = run(
        &root,
        &[
            "artifact",
            "update",
            id,
            "--revision",
            revision,
            "--title",
            "Stale",
            "--json",
        ],
    );
    assert!(!stale.status.success());
    assert!(stale.stdout.is_empty());
    let mut current = added;
    for command in ["resolve", "reopen", "delete"] {
        current = json(
            &root,
            &[
                "artifact",
                "comment",
                command,
                id,
                &comment,
                "--revision",
                current["revision"].as_str().unwrap(),
                "--json",
            ],
        );
        if command != "delete" {
            assert_eq!(
                current["artifact"]["comments"][0]["resolved"],
                command == "resolve"
            );
        }
    }
    assert!(
        current["artifact"]["comments"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        current["artifact"]["content"],
        created["artifact"]["content"]
    );
    let updated = json(
        &root,
        &[
            "artifact",
            "update",
            id,
            "--revision",
            current["revision"].as_str().unwrap(),
            "--repository",
            "other",
            "--json",
        ],
    );
    assert_eq!(updated["artifact"]["repository"], "other");
    assert_eq!(updated["artifact"]["status"], "todo");
    let done = json(
        &root,
        &[
            "artifact",
            "status",
            id,
            "--revision",
            updated["revision"].as_str().unwrap(),
            "--status",
            "done",
            "--json",
        ],
    );
    assert_eq!(done["artifact"]["status"], "done");
    let filtered = json(
        &root,
        &[
            "artifact",
            "list",
            "--repository",
            "other",
            "--status",
            "done",
            "--json",
        ],
    );
    assert_eq!(filtered["artifacts"].as_array().unwrap().len(), 1);
    let todo = json(
        &root,
        &[
            "artifact",
            "list",
            "--repository",
            "other",
            "--status",
            "todo",
            "--json",
        ],
    );
    assert!(todo["artifacts"].as_array().unwrap().is_empty());
}
#[test]
fn artifact_workflows_validate_kind_and_omit_status_for_other_kinds() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("data");
    let repository = root.join("portable/repositories/repo");
    fs::create_dir_all(&repository).unwrap();
    fs::write(repository.join("repository.json"), "{}").unwrap();
    let content = temp.path().join("content.md");
    fs::write(&content, "# Artifact").unwrap();
    for (kind, statuses) in [
        ("plan", &["todo", "in-progress", "done"][..]),
        ("rfc", &["to-review", "approved", "implemented"][..]),
        ("note", &[][..]),
        ("review", &[][..]),
        ("tutorial", &[][..]),
    ] {
        let args = [
            "artifact",
            "create",
            "--repository",
            "repo",
            "--title",
            kind,
            "--kind",
            kind,
            "--content-file",
            content.to_str().unwrap(),
            "--json",
        ];
        let mut current = json(&root, &args);
        assert_eq!(
            current["artifact"].get("status").is_some(),
            !statuses.is_empty()
        );
        if let Some(default) = statuses.first() {
            assert_eq!(current["artifact"]["status"], default.replace('-', "_"));
        }
        for status in statuses {
            current = json(
                &root,
                &[
                    "artifact",
                    "status",
                    current["artifact"]["id"].as_str().unwrap(),
                    "--revision",
                    current["revision"].as_str().unwrap(),
                    "--status",
                    status,
                    "--json",
                ],
            );
            assert_eq!(current["artifact"]["status"], status.replace('-', "_"));
        }
        let id = current["artifact"]["id"].as_str().unwrap();
        let invalid = if kind == "plan" { "approved" } else { "todo" };
        for command in ["status", "update"] {
            let failed = run(
                &root,
                &[
                    "artifact",
                    command,
                    id,
                    "--revision",
                    current["revision"].as_str().unwrap(),
                    "--status",
                    invalid,
                    "--json",
                ],
            );
            assert!(!failed.status.success());
            assert!(failed.stdout.is_empty());
            assert!(String::from_utf8_lossy(&failed.stderr).contains("not supported"));
        }
        assert_eq!(json(&root, &["artifact", "get", id, "--json"]), current);
        let summary = run(&root, &["artifact", "get", id]);
        assert!(summary.status.success());
        let summary = String::from_utf8(summary.stdout).unwrap();
        assert_eq!(summary.contains("] ["), !statuses.is_empty());
        let mut invalid_create = args.to_vec();
        invalid_create.extend(["--status", invalid]);
        assert!(!run(&root, &invalid_create).status.success());
    }
    let implemented = json(
        &root,
        &["artifact", "list", "--status", "implemented", "--json"],
    );
    assert_eq!(implemented["artifacts"].as_array().unwrap().len(), 1);
    assert_eq!(implemented["artifacts"][0]["artifact"]["kind"], "rfc");
}

#[test]
fn task_commands_are_removed_and_repository_is_required() {
    let temp = tempfile::tempdir().unwrap();
    for family in ["task", "subtask"] {
        assert_eq!(run(temp.path(), &[family, "list"]).status.code(), Some(2));
    }
    assert_eq!(
        run(
            temp.path(),
            &[
                "artifact",
                "create",
                "--title",
                "Plan",
                "--kind",
                "plan",
                "--content-file",
                "-"
            ]
        )
        .status
        .code(),
        Some(2)
    );
    let help = run(temp.path(), &["--help"]);
    let text = String::from_utf8(help.stdout).unwrap();
    assert!(!text.contains("subtask"));
    assert!(text.contains("artifact"));
    assert!(text.contains("session"));
}

#[test]
fn desktop_block_feedback_is_discoverable_and_resolvable_headlessly() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let dir = root.join("portable/artifacts/art-23456789");
    fs::create_dir_all(&dir).unwrap();
    // A desktop-produced artifact fixture. CLI reads the same metadata record.
    let metadata = serde_json::json!({
        "schemaVersion": 6, "id": "art-23456789", "title": "Block feedback", "kind": "note",
        "createdAt": 1, "updatedAt": 1,
        "comments": [{"id": "comment-23456789", "body": "Explain this", "resolved": false,
            "anchor": {"kind": "block", "start": 9, "end": 19, "startLine": 3, "endLine": 3,
                "source": "Paragraph.", "prefix": "# Title\n\n", "suffix": "\n", "quote": "Paragraph",
                "outdated": false}}]
    });
    fs::write(
        dir.join("artifact.md"),
        format!("---\n{metadata}\n---\n# Title\n\nParagraph.\n"),
    )
    .unwrap();
    let listed = json(
        root,
        &[
            "artifact",
            "comment",
            "list",
            "art-23456789",
            "--open",
            "--json",
        ],
    );
    assert_eq!(listed["comments"][0]["anchor"]["kind"], "block");
    assert_eq!(listed["comments"][0]["anchor"]["source"], "Paragraph.");
    assert_eq!(listed["comments"][0]["anchor"]["outdated"], false);
    let resolved = json(
        root,
        &[
            "artifact",
            "comment",
            "resolve",
            "art-23456789",
            "comment-23456789",
            "--revision",
            listed["revision"].as_str().unwrap(),
            "--json",
        ],
    );
    assert!(
        resolved["artifact"]["comments"][0]["resolved"]
            .as_bool()
            .unwrap()
    );
    assert!(
        json(
            root,
            &[
                "artifact",
                "comment",
                "list",
                "art-23456789",
                "--open",
                "--json"
            ]
        )["comments"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        json(
            root,
            &[
                "artifact",
                "comment",
                "list",
                "art-23456789",
                "--resolved",
                "--json"
            ]
        )["comments"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(
        !run(
            root,
            &[
                "artifact",
                "comment",
                "list",
                "art-23456789",
                "--open",
                "--resolved"
            ]
        )
        .status
        .success()
    );
    assert!(
        !run(
            root,
            &[
                "artifact",
                "comment",
                "reopen",
                "art-23456789",
                "comment-23456789",
                "--revision",
                listed["revision"].as_str().unwrap(),
                "--json"
            ]
        )
        .status
        .success()
    );
}

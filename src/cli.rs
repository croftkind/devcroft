//! Command-line surface; headless resources contract in `docs/resources.md`.
//!
//! `devcroft app [--checkout <path>]` boots the GUI; bare `devcroft`
//! prints help. Repository discovery and artifact commands are headless
//! shared-store operations. Local review comments also have headless
//! list/lifecycle commands.
//! Sync and live-UI ref/focus commands remain future work.
//! Dispatch in `main()` precedes GPUI initialization.

use std::path::PathBuf;

use anyhow::{Context as _, Result};
use clap::{Parser, Subcommand};

pub(crate) mod resources;
pub(crate) mod review;

/// First-class CLI: `devcroft app` starts the workspace; future subcommands
/// reuse this parser without changing the dispatch shape in `main()`.
#[derive(Debug, Parser)]
#[command(name = "devcroft", version, about = "Devcroft workspace and agent CLI")]
#[command(arg_required_else_help = true)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Command,
}

/// GUI entry points and headless resources, with resource handlers under
/// `src/commands/`. `GitStatus` is a headless diagnostic in
/// the spirit of the planned `status | doctor` command that itemizes what
/// the header dot sees, for loader-vs-CLI disagreements.
#[derive(Debug, Subcommand, PartialEq, Eq)]
pub(crate) enum Command {
    /// Boot the GPUI workspace, optionally opening a Markdown reference.
    App(AppArgs),
    /// Print what the header git dot sees for a checkout: branch, tracked
    /// dirtiness, and the first few itemized gix status entries, plus the
    /// `git status --porcelain` line count for comparison.
    GitStatus(GitStatusArgs),
    /// Discover portable repository keys (no local checkout required).
    Repository(resources::RepositoryArgs),
    /// Discover agent session identities for artifact origin links.
    Session(resources::RepositoryArgs),
    /// Manage standalone Markdown artifacts without a running desktop.
    Artifact(resources::ArtifactArgs),
    /// List and manage local branch-pair review comments.
    Review(review::ReviewArgs),
    /// Install, inspect, or remove the bundled agent skill.
    Skill(crate::agent_skill::Args),
}

/// Arguments for `devcroft app`.
#[derive(Debug, Clone, PartialEq, Eq, clap::Args)]
pub(crate) struct AppArgs {
    /// Checkout to open. Defaults to the current directory; terminal tabs,
    /// the project header, git status, and the Review projection all root
    /// here.
    #[arg(long, value_name = "PATH")]
    pub(crate) checkout: Option<PathBuf>,
    /// Open a portable repository, artifact, or session reference after startup.
    #[arg(long, value_name = "URL")]
    pub(crate) open_reference: Option<crate::markdown_references::Reference>,
}

/// Arguments for `devcroft git-status`.
#[derive(Debug, Clone, PartialEq, Eq, clap::Args)]
pub(crate) struct GitStatusArgs {
    /// Checkout to inspect. Defaults to the current directory.
    #[arg(long, value_name = "PATH")]
    pub(crate) checkout: Option<PathBuf>,
    /// Maximum itemized gix entries to print.
    #[arg(long, value_name = "N", default_value_t = 30)]
    pub(crate) limit: usize,
}

/// Resolve the working directory for `app`: explicit `--checkout` wins,
/// else the process current directory.
pub(crate) fn resolve_working_directory(checkout: Option<PathBuf>) -> Result<PathBuf> {
    match checkout {
        Some(path) => {
            if !path.is_dir() {
                anyhow::bail!("checkout is not a directory: {}", path.display());
            }
            Ok(path)
        }
        None => std::env::current_dir().context("resolving current directory for app"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_with_no_checkout_parses() {
        let cli = Cli::try_parse_from(["devcroft", "app"]).unwrap();
        assert_eq!(
            cli.command,
            Command::App(AppArgs {
                checkout: None,
                open_reference: None
            })
        );
    }

    #[test]
    fn app_checkout_parses() {
        let cli = Cli::try_parse_from(["devcroft", "app", "--checkout", "/tmp/work"]).unwrap();
        assert_eq!(
            cli.command,
            Command::App(AppArgs {
                checkout: Some(PathBuf::from("/tmp/work")),
                open_reference: None,
            })
        );
    }

    #[test]
    fn app_reference_is_validated_before_desktop_startup() {
        let cli = Cli::try_parse_from([
            "devcroft",
            "app",
            "--open-reference",
            "devcroft:repository/backend",
        ])
        .unwrap();
        let Command::App(args) = cli.command else {
            panic!("expected app")
        };
        assert_eq!(
            args.open_reference,
            Some(crate::markdown_references::Reference::Repository(
                "backend".into()
            ))
        );
        assert!(
            Cli::try_parse_from(["devcroft", "app", "--open-reference", "https://example.com"])
                .is_err()
        );
    }

    #[test]
    fn standalone_preview_is_not_a_command() {
        let error = Cli::try_parse_from(["devcroft", "preview", "README.md"]).unwrap_err();
        assert_eq!(error.kind(), clap::error::ErrorKind::InvalidSubcommand);
    }

    #[test]
    fn git_status_parses_with_defaults() {
        let cli = Cli::try_parse_from(["devcroft", "git-status"]).unwrap();
        assert_eq!(
            cli.command,
            Command::GitStatus(GitStatusArgs {
                checkout: None,
                limit: 30,
            })
        );
    }

    #[test]
    fn git_status_parses_checkout_and_limit() {
        let cli = Cli::try_parse_from([
            "devcroft",
            "git-status",
            "--checkout",
            "/tmp/work",
            "--limit",
            "5",
        ])
        .unwrap();
        assert_eq!(
            cli.command,
            Command::GitStatus(GitStatusArgs {
                checkout: Some(PathBuf::from("/tmp/work")),
                limit: 5,
            })
        );
    }

    #[test]
    fn bare_invocation_prints_help() {
        let error = Cli::try_parse_from(["devcroft"]).unwrap_err();
        // `arg_required_else_help` surfaces missing-subcommand as help text.
        let rendered = error.to_string();
        assert!(
            rendered.contains("Usage"),
            "expected usage, got: {rendered}"
        );
    }

    #[test]
    fn checkout_must_be_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        let ok = resolve_working_directory(Some(dir.path().to_path_buf())).unwrap();
        assert_eq!(ok, dir.path());

        let missing = dir.path().join("no-such-dir");
        let error = resolve_working_directory(Some(missing.clone())).unwrap_err();
        assert!(
            error.to_string().contains(&missing.display().to_string()),
            "error should name the bad path, got: {error:#}"
        );
    }

    #[test]
    fn missing_checkout_defaults_to_current_dir() {
        let resolved = resolve_working_directory(None).unwrap();
        assert_eq!(resolved, std::env::current_dir().unwrap());
    }
}

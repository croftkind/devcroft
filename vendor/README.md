# GPUI Base tab focus patch

`gpui-base/` contains the published `gpui-base` 0.10.0 crate, licensed under
Apache-2.0 (see its `LICENSE-APACHE`). Its published, normalized `Cargo.toml` is
unchanged. Registry installation metadata, the dependency lockfile and the
original pre-publication manifest are omitted.

- Source: <https://crates.io/crates/gpui-base/0.10.0>
- Upstream commit: `f697c3133d55c64fa8a5b67926147fb3dadd276d`
- Registry crate SHA-256:
  `4f4e5aff50636223b268a687ad5788ec54fa77c8e57ca9e53d158f19e0425d95`

The only source change is in `src/tabs.rs`: clicking an enabled tab explicitly
focuses its internal roving handle before stopping mouse-down propagation.
Upstream's propagation stop skips the normal `Div` focus handler, leaving focus
outside the tab list and breaking subsequent arrow/Space/Enter navigation.
The patch honors `default_prevented` so nested controls retain their focus,
skips disabled tabs and prevents the default only after claiming focus.

The application regression
`workspace::tests::git_header_tabs_switch_modes_and_preserve_workspace_tab`
exercises click → Left → Space → Right → Enter, including manual activation and
preservation of the active workspace tab. Run it on both GPUI backends.

Remove this directory and the root `[patch.crates-io]` entry after adopting an
upstream release that fixes click-to-keyboard tab focus and passes that
regression on both backends. Refresh the lockfile to restore the registry source
and checksum for `gpui-base`.

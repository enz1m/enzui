//! Version-control awareness for file managers and project views.
//!
//! [`git`] reads a Git repository's state from its files alone: where the
//! repository is, which branch is checked out, what changed. It never runs
//! `git` or anything a repository configures, so opening a folder cannot
//! execute a hook, a filter, an fsmonitor command or a credential helper,
//! whoever wrote the repository's config.

pub mod git;

# Contributing to enzUI

Start with [platform prerequisites](docs/platform-support.md),
[the dependency guide](docs/dependencies.md) and [the documentation map](docs/README.md).
The repository includes a Rust toolchain file and pins external forks to public
Git revisions. Cargo fetches them normally.

Development, formatting, lint and release builds use Rust 1.99.1. The workspace
keeps `rust-version = "1.95"`; CI separately checks that minimum with
`cargo +1.95.0 wcheck --locked` and builds the App example. New code and
dependency updates must preserve that compatibility.

## Development commands

```sh
cargo wbuild --locked
cargo wcheck --locked
cargo wt --locked --lib button
cargo wt --locked --test fields search_field
cargo wclippy
cargo fmt --all -- --check
cargo doc --locked --workspace --all-features --no-deps
```

The check, test and lint aliases share the development feature graph;
`wbuild` uses workspace defaults. No helper installation is needed. Keep filtered test runs during development; the full suite is
large and some native tests need a display, session bus or desktop service.
The [test guide](docs/testing.md) describes the harnesses.
See [build performance](docs/build-performance.md) for cache reuse, consumer
development profiles, ThinLTO and the optional PGO workflow.

`wbuild` uses default features. Check, test and Clippy aliases intentionally
enable all features, including internal test seams. Install their native build
prerequisites and keep those features out of shipping applications.

## Changes

- Fix a shared defect where it lives. Search [widgets](docs/widgets.md) and
  [core capabilities](docs/core-capabilities.md) before introducing another primitive.
- Add tests for observable behavior. Confirm a new regression test fails when
  its mechanism is disabled; a source string or builder field is rarely enough.
- Keep tests independent of private repositories and machine-specific paths.
- Prefer a small regression suite for stable behavior: input routing, identity,
  focus, rendering, resource ownership, persistence and security boundaries.
  Use tables or a few related scenarios for the same invariant. Avoid tests
  that only repeat a setter, pin a cosmetic default, census source spelling,
  or preserve the scaffolding of one past fix.
- Give lint exceptions a reason and unsafe blocks a `// SAFETY:` explanation.
- Describe current behavior in comments and public docs. Keep issue chronology
  and review notes out of implementation comments.
- Update the changelog for user-visible changes. The generated indexes have
  [regeneration commands](docs/README.md); their consistency tests stay in the suite.

Dependency changes must keep all crates from each fork on one URL and revision.
Change forks in their own repositories and update the pins here; do not edit
Cargo's source cache. Root patch tables do not propagate to consumers, so update
the dependency guide and check an independent consumer when routing changes.

This is a Git-source release. `publish = false` stays until a separate crates.io
packaging review resolves the Git dependency and bundled native-source requirements.

Contributions are under the project's [MIT license](LICENSE). Preserve the
notices of bundled third-party material.

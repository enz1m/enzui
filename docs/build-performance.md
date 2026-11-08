# Build speed, cache reuse and release optimization

Use `cargo check` while editing, a normal development build to try the app, and
release builds when measuring runtime performance. The workspace aliases are
plain Cargo commands; they do not install tools or delete caches. Cargo chooses
build parallelism automatically. The repository imposes no CPU, memory or disk
reserve limits; personal limits belong in the environment or user configuration.

## Development builds

The workspace development profile leaves editable workspace crates unoptimized
with incremental compilation enabled. Third-party dependencies use optimization
level 2 and no debug information or incremental state. This keeps rendering dependencies usable while
avoiding full dependency debug data. Workspace crates keep line tables for
backtraces; test builds omit debug information.

```sh
cargo wbuild --locked --example app
cargo wcheck --locked
cargo wt --locked --test fields search_field
```

`wcheck`, `wt` and `wclippy` intentionally use all features for framework
verification. Ordinary app builds should enable only their shipping features.
Run workspace checks, Clippy and documentation gates before linking examples
when verifying a framework change. CI uses that order so lint or documentation
failures are caught before an expensive executable build.

Treat those as separate build graphs; enabling every framework test seam in an
application is not a cache optimization.

Cargo takes profiles from the root being built, so these settings do not travel
with an enzUI dependency. A consumer application can use:

```toml
[profile.dev]
opt-level = 0
incremental = true
debug = "line-tables-only"

[profile.dev.package."*"]
opt-level = 2
debug = 0
incremental = false

# Use these overrides when editing enzUI alongside the app. A path dependency
# otherwise falls under the optimized dependency wildcard above.
[profile.dev.package.enzui-widgets]
opt-level = 0
incremental = true

[profile.dev.package.enzui-core]
opt-level = 0
incremental = true

[profile.dev.package.enzui-theme]
opt-level = 0
incremental = true

[profile.test]
debug = 0
```

Keep profiling and production performance measurements on release builds.
Unoptimized layout or model code can cost more CPU even when its dependencies
are optimized. For variable inspection, temporarily raise the editable crate's
debug setting; full debug data is more expensive than line tables.

The pinned Rust toolchain already uses bundled LLD by default on `x86_64-unknown-linux-gnu`.
Adding an extra linker flag to just one build entry point creates another cache
configuration without being necessary to select LLD on that target.

## Toolchains and warning policy

The development and release toolchain is pinned to Rust 1.99.1. The minimum
supported compiler remains 1.95, verified in a separate CI job. Use the pinned
compiler for normal work so formatting, lints and build artifacts agree.

On the pinned toolchain, `CARGO_BUILD_WARNINGS=deny cargo wcheck --locked`
rejects lint warnings without adding `-Dwarnings` to `RUSTFLAGS`. The `wclippy`
alias uses the same Cargo warning policy. Switching this warning policy does
not require new compiler artifacts, and cached warnings still fail the gate.
This Cargo setting requires 1.97 or newer; MSRV checks use 1.95 without it.

## Why targets grow

Cargo already reuses fresh dependencies. A normal source edit replaces the
outputs for that unit. It does not require a new dependency tree on every edit.
Persistent growth usually comes from different compiler versions, flags,
profiles, features, package versions and source locations, plus incremental
state and separate test executables.

Use the same pinned toolchain, dependency sources, lockfile versions, profiles
and flags across entry points. Different path copies of a framework are distinct
package identities. Several apps using the same Git URL and revision can share
that identity; copying identical source into each app does not achieve that.
A common source identity still needs compatible features and compiler settings.

A shared target directory helps compatible builds reuse artifacts:

```sh
export CARGO_TARGET_DIR="$HOME/.cache/enzui-target"
cargo build --locked
```

Set it in your development environment, not as an absolute path committed to the
framework. Keep the same value for the editor, terminal and build scripts.
Serial builds in one directory favor disk reuse; multiple build lanes trade
additional caches for parallelism. One shared directory alone does not unify
independent apps' dependency sources or feature sets.

Do not alternate `RUSTFLAGS`, `CARGO_INCREMENTAL`, debug overrides or linker
settings casually. `CARGO_INCREMENTAL=0` disables incremental compilation even
when the manifest enables it. Use `cargo build --timings` to inspect a slow
build and `--message-format=json` to distinguish fresh and rebuilt units.

## Cleanup and bounded compiler caches

Stable Cargo's automatic global cache collection concerns downloaded sources;
it does not remove stale compiled variants from target directories. Deleting
all old-looking files after every build can discard a variant another app uses.
Do not use `cargo clean` as part of the normal edit loop.

Keep recently used variants and collect genuinely inactive ones periodically.
Any external collector must respect Cargo's build locks, preserve documentation
and final outputs, and account for reused files whose modification time did not
change. An age based on modification time alone cannot prove an artifact was
unused. Audit automatic cleanup on the filesystem and Cargo version you use.

`sccache` is an optional second-level compiler cache with a configurable size
limit. It can restore eligible library compilations after an old target cache
is retired. It does not bound `target/`, cache final executable links, or cache
incrementally compiled crates. Its cache and target storage both count toward
your disk budget. Prefer local incremental builds for frequently edited crates;
measure before adopting a compiler-cache wrapper for the whole workflow.

The framework does not install a cleanup service or impose a global target path.

## Release candidates and ThinLTO

The ordinary release profile remains available. An opt-in profile adds ThinLTO:

```sh
cargo wbuild --locked --profile release-lto --example app
```

The output is `target/release-lto/examples/app`, adjusted for your target path.
Consumer apps copy this profile into their own root manifest:

```toml
[profile.release-lto]
inherits = "release"
lto = "thin"
codegen-units = 16
```

Measure startup, frame pacing, CPU usage and binary size against ordinary
`--release`. ThinLTO can reduce binary size without improving a particular
workload's runtime. Different profiles keep separate output directories.
Keep unwind support unless the application has explicitly qualified aborting
panics; changing panic strategy is not a free optimization.

## Profile-guided optimization

PGO is supported by stable Rust. Build an instrumented release, exercise
representative workflows, merge the counters, then rebuild with those counters.
Use the same sources, compiler, target and optimization settings for both
stages. The matching Rust LLVM tools avoid profile-format mismatches.

The following is a template for a consumer app whose binary is named `my-app`.
Run it at the application's root. These temporary environment assignments apply
only to the build command, not to later development builds.

```sh
pgo_toolchain=1.99.1 # Use the application's pinned Rust version.
rustup component add llvm-tools-preview --toolchain "$pgo_toolchain"
pgo_host=$(rustc +"$pgo_toolchain" -vV | sed -n 's/^host: //p')
pgo_dir="$PWD/target-pgo"
mkdir -p "$pgo_dir/profiles"

CARGO_TARGET_DIR="$pgo_dir/generate" \
RUSTFLAGS="-Cprofile-generate=$pgo_dir/profiles" \
cargo +"$pgo_toolchain" build --locked --profile release-lto --target "$pgo_host" --bin my-app

LLVM_PROFILE_FILE="$pgo_dir/profiles/%m-%p.profraw" \
"$pgo_dir/generate/$pgo_host/release-lto/my-app"
# Exercise several real app workflows and exit normally to flush counters.

profdata="$(rustc +"$pgo_toolchain" --print sysroot)/lib/rustlib/$pgo_host/bin/llvm-profdata"
"$profdata" merge -o "$pgo_dir/merged.profdata" "$pgo_dir"/profiles/*.profraw

CARGO_TARGET_DIR="$pgo_dir/use" \
RUSTFLAGS="-Cprofile-use=$pgo_dir/merged.profdata -Cllvm-args=-pgo-warn-missing-function" \
cargo +"$pgo_toolchain" build --locked --profile release-lto --target "$pgo_host" --bin my-app
```

Use a fresh profiling directory for each source/toolchain campaign. Regenerate
profiles when changing compilers; LLVM profile data is not a cross-version cache. Explicit
`--target` keeps host build scripts and procedural macros outside the target
instrumentation flags. Check compiler warnings, correctness and held-out
workloads before shipping the executable from the `use` directory.

Train navigation, searching, editing, resizing and relevant media workflows,
not only startup. Rust PGO does not optimize GPU shaders or automatically
instrument native dependencies such as GStreamer or WPE. After staging the
validated final executable, retire the disposable instrumentation build with
Cargo's `clean --target-dir` command; retain normal development caches.

## References

- [Cargo profiles](https://doc.rust-lang.org/cargo/reference/profiles.html)
- [Cargo build cache](https://doc.rust-lang.org/cargo/reference/build-cache.html)
- [Cargo cache configuration](https://doc.rust-lang.org/cargo/reference/config.html#cache)
- [Rust's default LLD linker](https://blog.rust-lang.org/2025/09/18/Rust-1.90.0/)
- [Rust PGO workflow](https://doc.rust-lang.org/rustc/profile-guided-optimization.html)
- [sccache Rust limitations](https://github.com/mozilla/sccache/blob/main/docs/Rust.md)

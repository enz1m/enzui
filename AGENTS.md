# enzUI contributor notes

enzUI is a Linux GUI framework built on iced. Fix defects in the shared framework
and prefer existing primitives over application-specific copies.

- Read [CONTRIBUTING.md](CONTRIBUTING.md) and the relevant public documentation.
- Use the portable workspace aliases in `.cargo/config.toml`. Keep Cargo in the
  foreground, reuse its target directory and avoid unnecessary cache deletion.
- Filter tests by target and name: `cargo wt --test fields search_field` or
  `cargo wt --lib button`. Verify the filter actually ran tests.
- Confirm new behavioral tests fail when the real mechanism is disabled, then
  restore it and pass. Test routing, paint and semantics through the actual path.
- Search `docs/widgets.md` and `docs/core-capabilities.md` by intent before adding
  a widget or a model. The widget and token JSON indexes support tooling.
- Every lint exception needs a reason; every unsafe block needs a `// SAFETY:`
  comment. Documentation describes current caller-visible behavior.
- Keep external dependencies on the public revisions in `docs/dependencies.md`.
  Never edit Cargo's cached checkouts. Do not assume a sibling development repository.
- Keep development-only observation and fault-injection features out of production
  defaults. Run checks appropriate to the change and state untested native paths.

```sh
cargo wcheck --locked
cargo wt --locked --test fields search_field
cargo wclippy
cargo fmt --all -- --check
```

The code is under `crates/`, public guides under `docs/`, and the bundled native
media source and provenance under `vendor/gst-curl/`.

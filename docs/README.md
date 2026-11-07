# enzUI documentation

Start with these two pages.

| page | answers |
|---|---|
| **[building-an-app.md](building-an-app.md)** | *How does this framework work?* The complete notes App example: state, editing, the app shell, window gestures and live themes. |
| **[widgets.md](widgets.md)** | *What already exists?* Every tier-1 widget, grouped by task, with its `enz_*` constructor, `Enz*` builder type and rustdoc link. |

Read **building-an-app** first if you have not written an iced app before, and
keep **widgets** open while you build: most of the cost of a missing widget is
rebuilding one that already exists.

## Reference

[Dependencies and consumer setup](dependencies.md) records all public fork pins
and the Cargo overrides needed by desktop applications.


| page | answers |
|---|---|
| **[core-capabilities.md](core-capabilities.md)** | *What exists that is not a widget?* Models, reducers, geometry resolvers and policy layers, indexed by the intent term you would search for. |
| **[theme-file.md](theme-file.md)** | *How does a user restyle every app?* The shared `enzui/theme.toml`: precedence, the light/dark schema, inheritance, diagnostics and runtime adoption. |
| **[localization.md](localization.md)** | App and framework translations, Fluent catalogs, language switching, CLDR formatting and RTL direction. |
| **[styling.md](styling.md)** | *How do I configure appearance in code?* `EnzTheme`, the shared scales, snapshot precedence and token roles. |
| **[tokens.md](tokens.md)** | *How do I restyle one widget?* Every `*Tokens` struct and its public fields. |
| **[colored-controls.md](colored-controls.md)** | How a coloured fill chooses readable text and icon ink. |
| **[text-shaping.md](text-shaping.md)** | When text uses basic or advanced shaping. |
| **[scrollbar-hit-band.md](scrollbar-hit-band.md)** | The scrollbar grab band and the constants derived from it. |
| **[performance-observer.md](performance-observer.md)** | Measuring input-to-frame latency with iced's opt-in `performance-observer` feature. |
| **[native-drag-and-drop.md](native-drag-and-drop.md)** | Dragging data, text and windows to and from other applications on Wayland: the runtime, accepting and finishing drops, starting a drag, drag icons and window drags. |
| **[platform-support.md](platform-support.md)** | Build prerequisites with package names, Wayland and X11 differences, and renderer selection (`ICED_BACKEND`). |
| **[wayland-only.md](wayland-only.md)** | Building a client without iced's X11 paths. |
| **[packaging.md](packaging.md)** | One application identity for the desktop entry, AppStream metainfo, icon, D-Bus service and search provider files, the window app id, and a Flatpak manifest. |
| **[logging.md](logging.md)** | Framework diagnostics through the `log` facade: targets, the default stderr logger and `RUST_LOG`, and installing your own logger. |
| **[build-performance.md](build-performance.md)** | Development profiles, shared caches, cleanup, ThinLTO and profile-guided optimization. |
| **[testing.md](testing.md)** | Testing an application headlessly: simulated input, focus, the accessibility tree, the accessibility audit, pixels, scoped preferences and locale. |
| **[features.md](features.md)** | Every Cargo feature of every enzUI crate, with test-only features marked. |
| **[developer-tools.md](developer-tools.md)** | iced's devtools and Comet, the `ENZUI_*` environment switches, and accessibility inspection. |
| **[web-engine-deployment.md](web-engine-deployment.md)** | Installing an app that embeds the WPE web engine. |

The crate documentation (`cargo doc --workspace --all-features --no-deps`) carries the same widget
catalog at the root of `enzui-widgets` and the capability index at the root
of `enzui-core`, and every crate under `crates/` has a `README.md`
saying what it is for.

## Generated pages

`widgets.md`, `core-capabilities.md` and `tokens.md` are generated, and a
test fails when any of them disagrees with its source, so edit the source and
regenerate. The generators turn rustdoc links into links to source files, so
the same text reads in `cargo doc` and on a forge. Catalog contract tests check
the generated source links; review the links in other guides and README files
when changing documentation.

- `core-capabilities.md` comes from `crates/enzui-core/src/capability_index.md`,
  which is also the `enzui-core` crate documentation:
  `ENZUI_WRITE_CAPABILITY_DOC=1 cargo wt --test capability_index docs_core_capabilities`

- `widgets.md` comes from the `# Widget catalog` block at the top of
  `crates/enzui-widgets/src/lib.rs`:
  `ENZUI_WRITE_WIDGET_DOC=1 cargo wt --test contracts widget_catalog_page`
- `tokens.md` comes from the public token structs in
  `crates/enzui-widgets/src`:
  `ENZUI_WRITE_TOKENS_DOC=1 cargo wt --test contracts token_index`

The [App example](../crates/enzui-widgets/examples/app.rs) is the runnable
source for `building-an-app.md`. The [Gallery](../crates/enzui-widgets/examples/gallery/main.rs)
shows individual widget variants. Those are the two runnable examples.
Widget catalog snippets live under the test fixtures and compile as part of
their contract target; regeneration checks their agreement with the page.

A few catalog entries carry a **Wiring:** sentence. It marks a widget where the
obvious call renders correctly but behaves wrongly unless the caller also wires
something, such as feeding a list's scroll offset back.

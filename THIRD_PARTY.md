# Third-party notices

The enzUI source is MIT licensed. That grant does not replace the licenses of
the following bundled source, data, assets and fixtures. Keep their original
notices when redistributing those materials.

| Material | License / notice | Provenance |
| --- | --- | --- |
| Lucide and Feather-derived icon assets | ISC and MIT; [license files](crates/enzui-icons/LICENSES) | [Icon provenance](crates/enzui-icons/THIRD_PARTY.md) |
| Emoji data | [Unicode License V3](crates/enzui-widgets/src/emoji_picker/LICENSE-UNICODE) | [Emoji data README](crates/enzui-widgets/src/emoji_picker/README.md) |
| Bounded Curl media source | [MIT](vendor/gst-curl/LICENSE-MIT), British Broadcasting Corporation / Sam Hurst | [Source ledger](vendor/gst-curl/README.md), [input hashes](vendor/gst-curl/UPSTREAM.json) |
| Arsenal row-count test font | [SIL OFL 1.1](crates/enzui-widgets/tests/row_count_generation_font.OFL.txt) | The Arsenal Project Authors; used by font-load regressions |
| Enz Badge Alignment digit font | [Bitstream Vera / DejaVu notice](crates/enzui-widgets/tests/badge_alignment_font.LICENSE.txt) | Renamed digit subset of DejaVu Sans |
| Fira Sans and Inter font fixtures | [Fira OFL](crates/enzui-widgets/tests/fixtures/fonts/FiraSans-OFL.txt), [Inter OFL](crates/enzui-widgets/tests/fixtures/fonts/Inter-LICENSE) | [Fixture provenance](crates/enzui-widgets/tests/fixtures/fonts/README.md) |

External Rust and native dependencies are not copied into this tree. Their
licenses and asset notices remain in their pinned source packages. In
particular, iced embeds Iced-Icons and optionally Fira Sans; its own source
includes their notices. Application distributors should collect dependency
notices for their actual feature set; see [Packaging](docs/packaging.md).

# enzui-test-support

Helpers for enzUI test builds: source-tree gates and test fixtures. It is a
**dev-dependency only**; nothing here ships in an application binary, and it
reports failures by panicking.

| module | what |
|---|---|
| `hand_roll_scan` | the completeness check behind an app's anti-hand-rolling guard: walks `src/`, finds every file that builds a widget tree, and fails if the guard does not scan it or scans too little of it |
| `test_target_inventory` | the crate's Cargo integration test targets, derived from source, so a filtered run cannot silently match nothing |
| `sabotage_manifest` | checks that the tests an app's `sabotage.toml` names still exist |
| `strict_fs` | fail-closed directory walks and reads for source gates: an unreadable entry is a failure, not a skip |
| `chained_operation` | follows `Outcome::Chain` exactly as the iced runtime does, for driving widget operations in tests |
| `test_scratch` | disk-backed scratch directories, removed on drop |
| `fake_executable` | writes fake helper programs for tests that spawn them |
| `rust_syntax` | `syn`-based helpers for source-policy gates (feature `rust-syntax`) |

Source gates here follow one rule: a scan that matches nothing must fail, not
pass. Walkers carry floors on how many files they visited and how much of each
file they read, because a check that cannot fail looks exactly like one that
passes.

`src/bin/style_inventory.rs` lists styling candidates in a source tree for
review; its findings are not verdicts.

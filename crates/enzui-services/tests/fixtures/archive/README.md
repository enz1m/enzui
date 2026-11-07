# Archive extraction fixtures

Inputs for `enzui_services::archive` tests (`src/archive/tests.rs`). Tests
never run an archiver; these files are generated once and committed.

`make_fixtures.py` writes `safe.tar.bz2` (a tar with nested, duplicate,
executable, `..`, absolute, symlink, hardlink, FIFO and device entries). It is
covered by the repository's MIT licence:

    python3 -I make_fixtures.py .

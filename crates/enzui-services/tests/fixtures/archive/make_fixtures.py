import bz2, io, sys, tarfile
out = sys.argv[1]
def add(t, name, data=b"", kind=tarfile.REGTYPE, link="", mode=0o644):
    i = tarfile.TarInfo(name); i.type = kind; i.linkname = link; i.mode = mode; i.mtime = 0
    i.size = len(data) if kind == tarfile.REGTYPE else 0
    t.addfile(i, io.BytesIO(data) if kind == tarfile.REGTYPE else None)
buf = io.BytesIO()
with tarfile.open(fileobj=buf, mode="w", format=tarfile.GNU_FORMAT) as t:
    add(t, "nested/", kind=tarfile.DIRTYPE, mode=0o755)
    add(t, "nested/hello.txt", b"hello archive\n")
    add(t, "duplicate.txt", b"first")
    add(t, "duplicate.txt", b"second")
    add(t, "empty/", kind=tarfile.DIRTYPE, mode=0o755)
    add(t, "run.sh", b"#!/bin/sh\n", mode=0o755)
    add(t, "../escaped", b"no")
    add(t, "/absolute", b"no")
    add(t, "link", kind=tarfile.SYMTYPE, link="/etc/passwd")
    add(t, "hard", kind=tarfile.LNKTYPE, link="nested/hello.txt")
    add(t, "fifo", kind=tarfile.FIFOTYPE)
    add(t, "tty", kind=tarfile.CHRTYPE)
tar = buf.getvalue()
open(f"{out}/safe.tar.bz2", "wb").write(bz2.compress(tar))

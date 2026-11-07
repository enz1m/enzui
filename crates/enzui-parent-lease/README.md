# enzui-parent-lease

The lifetime rules for an XDG portal parent lease, with no Wayland or portal
dependency.

A parented portal request carries a `parent_window` handle that is only
meaningful while the exported protocol object behind it is alive. Every adopter
owes the same four things:

1. hold the lease for exactly as long as the request wants the parent
   relationship;
2. release it once, when the request settles or is cancelled;
3. retire it the moment the surface it names is replaced;
4. refuse a response that belongs to a lease it no longer holds.

`ParentLease` is that owner. It is generic over the lease type, so the rules are
tested without a compositor.

## Surface identity

`SurfaceIncarnation` is not minted here. Whoever creates the surface mints it,
at the moment of creation, and never reuses it; this crate only carries and
compares it. A counter kept beside the request could not tell a destroyed
surface from a later one that reuses its numeric id, and a window id names a
window, which outlives any single surface.

`enzui-wayland` adopts these rules for the real xdg-foreign export, and
`enzui-widgets` adopts them for portal requests.

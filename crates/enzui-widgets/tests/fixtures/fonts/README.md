# Font preference fixtures

These unmodified faces make the real font-preference routing regression
independent of a vendor checkout. They are test data, not runtime assets.

- Fira Sans Regular: copied from the pinned enzim-iced revision
  `67f991b7442cbd79821141069c3b864cc12413f2`, `graphics/fonts/FiraSans-Regular.ttf`.
  The Mozilla Foundation and Telefonica S.A.; SIL OFL 1.1 in `FiraSans-OFL.txt`.
- Inter Regular: copied from the pinned enzim-cosmic-text revision
  `ad474131e52c0fde5979f8def2889489b50a0480`, `fonts/Inter-Regular.ttf`.
  The Inter Project Authors; SIL OFL 1.1 in `Inter-LICENSE`.

| File | SHA-256 |
| --- | --- |
| `FiraSans-Regular.ttf` | `3dabf3d48bf4599f95cffd92f99ea426a014d5311f52a5eb5ec3af265e97cd97` |
| `Inter-Regular.ttf` | `3127f0b873387ee37e2040135a06e9e9c05030f509eb63689529becf28b50384` |

The Inter fixture contains the verified LFS object bytes, not a pointer file.

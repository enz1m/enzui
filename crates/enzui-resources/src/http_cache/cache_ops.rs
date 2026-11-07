//! `HttpCache`'s methods: opening the root, lookup, store, revalidation,
//! removal and eviction.

use super::*;

impl HttpCache {
    /// Open the cache under the XDG cache directory.
    ///
    /// Never fails: if the directory cannot be resolved, created, or secured,
    /// caching is disabled and every operation becomes a no-op. A read-only or
    /// sandboxed home must not break image loading. The root is provisioned
    /// through the shared [`ensure_private_dir`](enzui_security::secure_dir::ensure_private_dir)
    /// boundary: a directory created here is `0o700` regardless of the ambient
    /// umask, and an existing wrong-owner, group/world-writable or symlinked root
    /// is refused rather than trusted. A refusal disables caching and is
    /// reported by [`unavailable`](Self::unavailable).
    #[must_use]
    pub fn open(config: HttpCacheConfig) -> Self {
        if config.max_bytes == 0 {
            // `0` means no cache: do not even resolve a directory.
            return Self::unavailable_with(config, CacheUnavailable::Disabled);
        }
        let Some(base) = xdg_cache_root() else {
            return Self::unavailable_with(config, CacheUnavailable::NoCacheHome);
        };
        Self::at_secured_root(base.join(CACHE_SUBDIR), config)
    }

    /// Open at an explicit root, for tests and for applications that manage
    /// their own cache location.
    ///
    /// Like [`open`](Self::open), the root is created (nested, `0o700`) and then
    /// routed through the shared secure-ensure boundary: an unsafe root disables
    /// caching — reported by [`unavailable`](Self::unavailable) — instead of
    /// hosting entries in a directory another user can write.
    #[must_use]
    pub fn open_at(root: impl Into<PathBuf>, config: HttpCacheConfig) -> Self {
        if config.max_bytes == 0 {
            return Self::unavailable_with(config, CacheUnavailable::Disabled);
        }
        Self::at_secured_root(root.into(), config)
    }

    /// Provision `dir` through the shared boundary and retain the descriptor.
    ///
    /// Individual entry writes are mode `0o600` and use `O_NOFOLLOW`, while the
    /// shared boundary refuses a wrong-owner, world-writable, or symlinked leaf.
    /// Neither closes the *pathname substitution after adoption* window: a
    /// same-uid process could rename the verified leaf and
    /// plant a decoy directory or a symlink at the same path, and every later
    /// entry op that resolves that path from scratch would be redirected. So
    /// what is retained here is not a `PathBuf` — it is a [`SecureRoot`], whose
    /// `openat`/`renameat`/`unlinkat` methods run against the pinned inode via
    /// its retained `O_DIRECTORY | O_NOFOLLOW` fd.
    /// [`SecureDirRefusal::RootIdentityChanged`] is how a substituted root
    /// surfaces on an already-open cache.
    pub(super) fn at_secured_root(dir: PathBuf, config: HttpCacheConfig) -> Self {
        match SecureRoot::open(&dir) {
            Ok(root) => Self {
                root: Some(root),
                config,
                unavailable: None,
            },
            Err(refusal) => Self::unavailable_with(config, CacheUnavailable::Root(refusal)),
        }
    }

    /// A cache with no store and the typed reason why.
    const fn unavailable_with(config: HttpCacheConfig, why: CacheUnavailable) -> Self {
        Self {
            root: None,
            config,
            unavailable: Some(why),
        }
    }

    /// Whether caching is active. `false` for `max_bytes == 0` or an unusable
    /// directory.
    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        self.root.is_some()
    }

    /// Why caching was inactive at construction, or `None` when the cache
    /// opened successfully.
    ///
    /// `Some` exactly when [`is_enabled`](Self::is_enabled) is `false`. This is
    /// the *open-time* refusal only; a substituted root pathname after the
    /// handle is live surfaces separately via [`root_status`](Self::root_status).
    /// Splitting them keeps "could not open" (a boot-time policy signal)
    /// distinguishable from "the root has been substituted underneath"
    /// (a live-security finding).
    #[must_use]
    pub const fn unavailable(&self) -> Option<&CacheUnavailable> {
        self.unavailable.as_ref()
    }

    /// Live root status: `Ok(())` when the root pathname still resolves to the
    /// identity captured at open time, `Err` when a substitution has happened.
    ///
    /// This is how a live identity mismatch surfaces: an application logs a
    /// [`CacheUnavailable::Root`] with a
    /// [`SecureDirRefusal::RootIdentityChanged`] payload after a same-uid
    /// process renamed the verified leaf and put a decoy in its place. Every
    /// public op below performs the same check internally and degrades to a
    /// miss/false/failure; this accessor exposes the typed cause without
    /// requiring the caller to observe it as a side effect of an operation.
    ///
    /// Returns [`CacheUnavailable::Disabled`] when caching is off by
    /// configuration and the pre-open refusal (if any) otherwise, so callers
    /// have one accessor for "is anything wrong right now".
    ///
    /// # Errors
    ///
    /// [`CacheUnavailable::Disabled`] when caching is off, and the root's refusal otherwise.
    //
    // The return type is `Result`, which is itself `#[must_use]`, so an outer
    // `#[must_use]` on the function would double the annotation without adding
    // a message. Callers still cannot silently discard the verdict.
    pub fn root_status(&self) -> Result<(), CacheUnavailable> {
        match &self.root {
            None => Err(self
                .unavailable
                .clone()
                .unwrap_or(CacheUnavailable::Disabled)),
            Some(root) => match root.revalidate() {
                Ok(()) => Ok(()),
                Err(refusal) => Err(CacheUnavailable::Root(refusal)),
            },
        }
    }

    /// Total bytes currently stored, or `u64::MAX` when the census is incomplete.
    ///
    /// This backward-compatible convenience accessor is deliberately
    /// conservative: an enumeration/stat failure can never look like a small
    /// exact total or a satisfied byte budget. Call [`try_stored_bytes`](Self::try_stored_bytes)
    /// when the failure itself must be reported. A disabled cache has a
    /// complete empty census and returns `0`.
    #[must_use]
    pub fn stored_bytes(&self) -> u64 {
        self.try_stored_bytes().unwrap_or(u64::MAX)
    }

    /// Total bytes currently stored, with incomplete-census errors exposed.
    ///
    /// # Errors
    ///
    /// Returns the contextual directory-read, per-name stat, or live root
    /// identity error that made the census incomplete.
    pub fn try_stored_bytes(&self) -> io::Result<u64> {
        Ok(self.entries()?.iter().map(|entry| entry.size).sum())
    }

    /// Number of entries currently stored, or `usize::MAX` when the census is incomplete.
    ///
    /// This preserves the original infallible API without ever returning a
    /// partial count as exact. Call [`try_entry_count`](Self::try_entry_count)
    /// to distinguish and inspect an incomplete census. A disabled cache
    /// returns `0`.
    #[must_use]
    pub fn entry_count(&self) -> usize {
        self.try_entry_count().unwrap_or(usize::MAX)
    }

    /// Number of entries currently stored, with incomplete-census errors exposed.
    ///
    /// # Errors
    ///
    /// Returns the contextual directory-read, per-name stat, or live root
    /// identity error that made the census incomplete.
    pub fn try_entry_count(&self) -> io::Result<usize> {
        Ok(self.entries()?.len())
    }

    /// Look up `url`.
    ///
    /// Returns `None` for a miss, an unusable cache, an entry that fails
    /// validation, or a substituted root ([`root_status`](Self::root_status)
    /// then reports [`CacheUnavailable::Root`] with
    /// [`SecureDirRefusal::RootIdentityChanged`]). A corrupt entry is removed
    /// as a side effect so it cannot be re-read on every request. Every read
    /// goes through the retained root fd, so the miss on a substituted root
    /// never dereferences a decoy pathname.
    #[must_use]
    pub fn get<'k>(
        &self,
        key: impl Into<CacheKey<'k>>,
        request_headers: &[(String, String)],
    ) -> Option<CachedResponse> {
        let key = key.into();
        let root = self.root.as_ref()?;
        // Substitution gate: the pathname must still resolve to the identity
        // captured at open time, or every op is a fail-closed miss. The gate
        // is the *signal*; the syscalls below use `*at(fd, name, …)` and are
        // safe on the pinned inode either way.
        if root.revalidate().is_err() {
            return None;
        }
        // Two-step lookup, because on read the cache does not yet know which
        // headers this URL varies on.
        //
        // Step 1 reads the unvaried partition (method/URL/range/caller, no Vary).
        // For a response with no `Vary` that is the entry itself and the work is
        // done. For a varying response the entry stored there is a *pointer*: it
        // names the fields, and step 2 rebuilds the real key from the request's
        // values for exactly those fields — every repeated occurrence, in order.
        let base_material = canonical_material(&key, &[], &[]);
        let base_name = entry_filename_material(&base_material);
        let base = self.load(root, &base_name, &identity_digest_material(&base_material))?;
        if base.vary_names.is_empty() {
            return self.present(root, base, &base_name);
        }
        let material = canonical_material(&key, &base.vary_names, request_headers);
        let name = entry_filename_material(&material);
        let entry = self.load(root, &name, &identity_digest_material(&material))?;
        // THE POINTER IS PART OF THIS ENTRY'S REACHABILITY CHAIN, so serving
        // the varied entry must refresh the pointer's recency too.
        //
        // Eviction is LRU on mtime, and `present` below touches only the file
        // it serves. A varying URL therefore had a pointer whose mtime never
        // advanced after the store that created it, no matter how often the
        // URL was read — so the *hottest* varying entry in the cache offered
        // the *oldest* file, and eviction took the pointer first. The pointer
        // is a few hundred bytes, so unlinking it frees essentially nothing and
        // the loop continues to a real victim; what it does accomplish is to
        // make `get` above return `None` at step 1 forever, while the varied
        // body it named stays on disk as an unreachable orphan still charged
        // against `max_bytes`. Measured on a three-entry cache: the hot varying
        // entry became a permanent miss and its 638-byte body was stranded.
        //
        // Touching it only on a hit is deliberate: a step-2 miss is followed by
        // a fetch whose `store` rewrites both files anyway, and refreshing a
        // pointer that names no live entry would just retain garbage.
        let _ = root.touch_entry_mtime(&base_name);
        self.present(root, entry, &name)
    }

    /// The largest an entry file may be before it is refused unread.
    ///
    /// A legitimate entry is a small fixed header, a redacted display URL and
    /// validators, and a body already bounded by `max_entry_bytes` at store
    /// time. Anything past this ceiling is a replaced or corrupt file and must
    /// not be pulled into memory to find that out.
    pub(super) fn read_limit(&self) -> u64 {
        self.config
            .max_entry_bytes
            .saturating_add(MAX_ENTRY_METADATA_BYTES)
    }

    /// Read and validate one entry file, removing it if it does not validate.
    ///
    /// The open is `openat(root_fd, name, O_RDONLY | O_NOFOLLOW | O_NONBLOCK)`
    /// so a symlink at the entry name is refused in the kernel and a planted
    /// FIFO cannot hang the open. A corrupt or identity-mismatched entry is
    /// removed only while the name still denotes the exact inode this load read,
    /// so a concurrent writer's newer replacement published between the read and
    /// the unlink is left untouched.
    pub(super) fn load(
        &self,
        root: &SecureRoot,
        name: &OsStr,
        expected_identity: &[u8; IDENTITY_DIGEST_LEN],
    ) -> Option<StoredEntry> {
        let file = root.open_entry_read(name).ok()?;
        // Cleanup below binds to this inode, so the descriptor is captured and
        // held open through the unlink: an open fd pins the inode number so it
        // cannot be recycled under the revalidation.
        let observed = open_file_identity(&file).ok()?;
        let raw = read_bounded_from(&file, self.read_limit())?;
        let servable =
            decode_entry(&raw).filter(|candidate| &candidate.identity == expected_identity);
        if servable.is_none() {
            // A miss whether the bytes are corrupt/truncated or the stored
            // digest disagrees with this request's complete partition. Removing
            // the inode we read keeps the damage from being re-read forever
            // without deleting a replacement that no longer names it.
            #[cfg(test)]
            cleanup_race::run_corrupt();
            let _ = root.unlink_entry_if_identity(name, observed);
        }
        drop(file);
        servable
    }

    /// Turn a validated entry into a response, touching it for LRU purposes.
    pub(super) fn present(
        &self,
        root: &SecureRoot,
        entry: StoredEntry,
        name: &OsStr,
    ) -> Option<CachedResponse> {
        // Best-effort mtime touch; a failure only makes eviction less accurate.
        // The `touch_entry_mtime` open is `O_NOFOLLOW` so it cannot rewrite a
        // victim's mtime through a substituted symlink.
        let _ = root.touch_entry_mtime(name);
        let fresh = entry.is_fresh(SystemTime::now()) && !entry.no_cache;
        Some(CachedResponse {
            body: entry.body,
            etag: entry.etag,
            last_modified: entry.last_modified,
            fresh,
        })
    }

    /// Store a response, honouring `no-store` and the size ceiling.
    ///
    /// Returns whether the entry was written. Failure is not an error: an
    /// unwritable cache must not fail a load.
    pub fn store<'k>(
        &self,
        key: impl Into<CacheKey<'k>>,
        body: &[u8],
        freshness: &Freshness,
        request_headers: &[(String, String)],
    ) -> bool {
        let key = key.into();
        // `no-store` is checked first, before any path is built or file created.
        // Writing and then removing would already have exposed the bytes.
        if !freshness.may_store() {
            return false;
        }
        if body.len() as u64 > self.config.max_entry_bytes {
            return false;
        }
        let Some(root) = self.root.as_ref() else {
            return false;
        };
        // Fail closed on a substituted root before allocating an entry, so a
        // decoy at the pathname is never even the target of a temp write.
        if root.revalidate().is_err() {
            return false;
        }
        // Validate and cap the canonical Vary-name set before constructing the
        // canonical request material or encoding/writing either entry. This also
        // protects callers that construct the public `Freshness` struct directly
        // instead of using `from_headers`.
        let Some(vary_names) = normalized_vary_names(&freshness.vary) else {
            return false;
        };
        // One canonical partition drives both the filename and the persisted
        // identity: method, URL, range, caller partition, and every repeated
        // request-field occurrence for each Vary name, in order.
        let material = canonical_material(&key, &vary_names, request_headers);
        let name = entry_filename_material(&material);
        let identity = identity_digest_material(&material);
        let stored_at = SystemTime::now();
        let received_at = unix_seconds(stored_at);
        // Resolve the lifetime once, at store time, from max-age or Expires.
        let lifetime = freshness.lifetime_seconds(received_at);
        // `Age` is captured as it was on arrival. Combined with the residency
        // time computed on read, this is RFC 9111's apparent age.
        let initial_age = freshness.age.unwrap_or(0);
        let encoded = encode_entry(&StoredEntry {
            identity,
            display_url: key.display.to_owned(),
            body: body.to_vec(),
            etag: freshness.etag.clone(),
            last_modified: freshness.last_modified.clone(),
            lifetime,
            initial_age,
            no_cache: freshness.no_cache,
            // Only the field NAMES are persisted — the request-header VALUES
            // (which may be a bearer token or a session cookie) live only inside
            // the one-way `identity` digest above, never on disk.
            vary_names: vary_names.clone(),
            stored_at,
        });
        // A body within `max_entry_bytes` can still encode past `read_limit` if
        // its metadata (a long redacted display URL, or many Vary pairs) is
        // large. Refuse to write such an entry: `get` reads through `read_limit`
        // and would treat it as a miss on every request, so it would only burn
        // budget and re-download forever. Refusing here keeps store and load in
        // agreement.
        if encoded.len() as u64 > self.read_limit() {
            return false;
        }
        // Retain the published inode's identity so a later rollback removes only
        // the body this call published, never a concurrent writer's newer one.
        let Ok(published) = write_atomically(root, &name, &encoded) else {
            return false;
        };

        // For a VARYING response, also write a small pointer at the unvaried key.
        //
        // `get` cannot know which fields a URL varies on until it has read
        // something, so it reads the unvaried key first and uses the `vary` field
        // names it finds there to rebuild the real key. Without this pointer that
        // first read misses and a varying entry is unreachable — which is exactly
        // what the calibration test caught.
        //
        // The pointer carries the field NAMES (no values, no body), so it costs
        // a few dozen bytes and can never be mistaken for a servable entry:
        // `get` only ever reads `vary_names` from it.
        let base_material = canonical_material(&key, &[], &[]);
        let base_name = entry_filename_material(&base_material);
        if !vary_names.is_empty() && base_name != name {
            let pointer = encode_entry(&StoredEntry {
                identity: identity_digest_material(&base_material),
                display_url: key.display.to_owned(),
                body: Vec::new(),
                etag: None,
                last_modified: None,
                lifetime: None,
                initial_age: 0,
                no_cache: false,
                vary_names,
                stored_at,
            });
            if write_atomically(root, &base_name, &pointer).is_err() {
                // The real entry is now unreachable without its pointer, but the
                // rollback unlinks `name` only while it still denotes the inode
                // this store published: a concurrent writer's newer reachable
                // body is preserved rather than destroyed, an unproven orphan
                // being the safer outcome.
                #[cfg(test)]
                cleanup_race::run_rollback();
                let _ = root.unlink_entry_if_identity(&name, published);
                return false;
            }
        }

        // The response was published even if a concurrent filesystem failure
        // prevents proving/enforcing the aggregate budget. Eviction itself is
        // fallible and never reports a partial census as "within budget";
        // storage remains best-effort, so this write result retains its public
        // meaning and the next store retries eviction.
        let _ = self.evict_to_budget();
        true
    }

    /// Refresh an entry's stored timestamp after a `304 Not Modified`, so the
    /// cached bytes become fresh again without re-downloading them.
    ///
    /// Returns whether an entry was updated.
    pub fn mark_revalidated<'k>(
        &self,
        key: impl Into<CacheKey<'k>>,
        freshness: &Freshness,
        request_headers: &[(String, String)],
    ) -> bool {
        let key = key.into();
        let Some(root) = self.root.as_ref() else {
            return false;
        };
        if root.revalidate().is_err() {
            return false;
        }
        // A revalidation response is remote input too. Apply the same token and
        // unique-name ceiling as `store` before allocating canonical material or
        // opening/rewriting an existing entry.
        let Some(vary_names) = normalized_vary_names(&freshness.vary) else {
            return false;
        };
        let material = canonical_material(&key, &vary_names, request_headers);
        let name = entry_filename_material(&material);
        // Revalidation must apply the same complete-identity check as a read
        // before changing even one metadata field. Besides sharing the bounded
        // decode path, `load` verifies method, URL, range, caller partition and
        // every repeated Vary occurrence through the stored digest. A moved or
        // planted entry is removed as a miss, never refreshed into legitimacy.
        let Some(mut entry) = self.load(root, &name, &identity_digest_material(&material)) else {
            return false;
        };
        entry.stored_at = SystemTime::now();
        // A 304 restarts the clock: the revalidated response is current as of
        // now, so any age it accumulated upstream previously no longer applies.
        entry.initial_age = freshness.age.unwrap_or(0);
        // A 304 may carry updated directives; take them when present and keep
        // the existing ones otherwise.
        if let Some(lifetime) = freshness.lifetime_seconds(unix_seconds(entry.stored_at)) {
            entry.lifetime = Some(lifetime);
        }
        if freshness.etag.is_some() {
            entry.etag.clone_from(&freshness.etag);
        }
        if freshness.last_modified.is_some() {
            entry.last_modified.clone_from(&freshness.last_modified);
        }
        entry.no_cache = freshness.no_cache;
        write_atomically(root, &name, &encode_entry(&entry)).is_ok()
    }

    /// Remove the entry for `key` from the cache.
    ///
    /// Uses `unlinkat(root_fd, name)` — descriptor-relative, so a substituted
    /// root pathname cannot redirect the unlink to a decoy. Fails on a
    /// substituted root (revalidation refuses) rather than unlinking anything
    /// through the pinned inode when the caller does not expect it to still be
    /// the store.
    pub fn remove<'k>(&self, key: impl Into<CacheKey<'k>>) -> bool {
        let Some(root) = self.root.as_ref() else {
            return false;
        };
        if root.revalidate().is_err() {
            return false;
        }
        // The unvaried partition names the base entry (or, for a varying URL,
        // its pointer): the same canonical material with no Vary values.
        let base_name = entry_filename_material(&canonical_material(&key.into(), &[], &[]));
        root.unlink_entry(&base_name).is_ok()
    }

    /// Delete every entry, reporting what was removed and what could not be.
    ///
    /// A privacy-facing clear must not silently succeed while files remain: the
    /// outcome carries per-entry failures so the caller can decide policy.
    /// Fallible cache accounting ([`HttpCache::try_stored_bytes`]) returns an
    /// exact value when the remaining root can be fully enumerated, never a
    /// partial sum.
    pub fn clear(&self) -> ClearOutcome {
        let Some(root) = &self.root else {
            return ClearOutcome {
                removed: 0,
                failures: Vec::new(),
            };
        };
        // Fail closed on a substituted root: return one aggregate failure
        // attributed to the root, so a UI "Clear cached data" action reports
        // the real cause instead of "success, nothing to clear" — which is
        // exactly the misreport this typed-outcome path exists to prevent.
        if let Err(refusal) = root.revalidate() {
            return ClearOutcome {
                removed: 0,
                failures: vec![ClearFailure {
                    path: root.path().to_path_buf(),
                    kind: io::ErrorKind::PermissionDenied,
                    message: format!("cache root identity changed: {refusal}"),
                }],
            };
        }
        self.clear_with_census(root, retained_census)
    }

    /// Full pathname for a URL's cache entry, produced only for tests that
    /// need to inspect or seed files on disk.
    ///
    /// Production reads and writes go through [`SecureRoot`] and never touch a
    /// `PathBuf`; this is deliberately `#[cfg(test)]` so a future
    /// caller cannot fall back to the pathname view and reintroduce the
    /// substitution hole this seam closes.
    #[cfg(test)]
    pub(super) fn path_for(&self, url: &str) -> Option<PathBuf> {
        let root = self.root.as_ref()?;
        Some(
            root.path()
                .join(entry_filename_material(&base_material_for(url))),
        )
    }

    /// Existing entries, cheapest-metadata only, enumerated via `getdents` on
    /// the retained root fd so a substituted pathname cannot redirect the scan.
    /// An incomplete census is an error, never an empty vector.
    pub(super) fn entries(&self) -> io::Result<Vec<EntryStat>> {
        let Some(root) = &self.root else {
            return Ok(Vec::new());
        };
        self.census_entries_with(root, retained_census)
    }

    /// Evict least-recently-used entries until the total is within budget.
    ///
    /// The result is deliberately fallible: a failed census cannot prove the
    /// budget is satisfied. `store` remains best-effort and ignores this result
    /// after its own entry has been atomically published.
    pub(super) fn evict_to_budget(&self) -> io::Result<()> {
        let Some(root) = &self.root else {
            return Ok(());
        };
        self.evict_to_budget_with(root, retained_census)
    }

    pub(super) fn evict_to_budget_with<F>(&self, root: &SecureRoot, enumerate: F) -> io::Result<()>
    where
        F: FnOnce(&SecureRoot) -> io::Result<Vec<CensusEntry>>,
    {
        let mut entries = self.census_entries_with(root, enumerate)?;
        let mut total: u64 = entries.iter().map(|entry| entry.size).sum();
        if total <= self.config.max_bytes {
            return Ok(());
        }
        // Oldest use first. Filesystems commonly expose modification times at
        // a coarser resolution than the cache's nanosecond representation, so
        // several entries can tie. `read_dir` order is explicitly unspecified;
        // letting that order break ties made two equivalent cache directories
        // retain different response bytes. The digest filename is stable for a
        // request identity, so it is the canonical secondary key.
        canonicalize_eviction_order(&mut entries);
        for entry in entries {
            if total <= self.config.max_bytes {
                break;
            }
            let Some(name) = entry.path.file_name() else {
                continue;
            };
            match evict_one_entry(root, name, entry.size) {
                Ok(freed) => total = total.saturating_sub(freed),
                Err(error) => {
                    return Err(io::Error::new(
                        error.kind(),
                        format!(
                            "evict cache entry {} from secure root {}: {error}",
                            entry.path.display(),
                            root.path().display()
                        ),
                    ));
                }
            }
        }
        if total <= self.config.max_bytes {
            Ok(())
        } else {
            Err(io::Error::other(format!(
                "cache eviction in secure root {} ended at {total} bytes, above budget {}",
                root.path().display(),
                self.config.max_bytes
            )))
        }
    }

    pub(super) fn census_entries_with<F>(
        &self,
        root: &SecureRoot,
        enumerate: F,
    ) -> io::Result<Vec<EntryStat>>
    where
        F: FnOnce(&SecureRoot) -> io::Result<Vec<CensusEntry>>,
    {
        root.revalidate().map_err(|refusal| {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "enumerate cache entries in secure root {}: {refusal}",
                    root.path().display()
                ),
            )
        })?;
        Ok(enumerate(root)?
            .into_iter()
            .filter(|stat| {
                Path::new(&stat.name)
                    .extension()
                    .and_then(|extension| extension.to_str())
                    == Some("enzhc")
            })
            .map(|stat| EntryStat {
                path: root.path().join(&stat.name),
                size: stat.size,
                used_at: stat.used_at,
            })
            .collect())
    }

    pub(super) fn clear_with_census<F>(&self, root: &SecureRoot, enumerate: F) -> ClearOutcome
    where
        F: FnOnce(&SecureRoot) -> io::Result<Vec<CensusEntry>>,
    {
        let names = match enumerate(root) {
            Ok(entries) => entries,
            Err(error) => {
                return ClearOutcome {
                    removed: 0,
                    failures: vec![ClearFailure {
                        path: root.path().to_path_buf(),
                        kind: error.kind(),
                        message: error.to_string(),
                    }],
                };
            }
        };
        let mut removed = 0;
        let mut failures = Vec::new();
        for entry in names {
            match root.unlink_entry(&entry.name) {
                Ok(()) => removed += 1,
                Err(error) => failures.push(ClearFailure {
                    path: root.path().join(&entry.name),
                    kind: error.kind(),
                    message: error.to_string(),
                }),
            }
        }
        ClearOutcome { removed, failures }
    }
}

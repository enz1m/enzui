//! Watches for files that may not exist yet, through the nearest existing
//! ancestor of each one's directory.

use super::backend::{BoxHandle, DirectoryWatchBackend};
use super::sink::EventSink;
use super::types::WatchToken;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(crate) struct AncestorWatch {
    backend: Arc<dyn DirectoryWatchBackend>,
    targets: Vec<PathBuf>,
    watched: HashMap<PathBuf, BoxHandle>,
    sink: Arc<dyn EventSink>,
}

impl AncestorWatch {
    pub(crate) fn new(
        backend: Arc<dyn DirectoryWatchBackend>,
        files: impl IntoIterator<Item = PathBuf>,
        sink: Arc<dyn EventSink>,
    ) -> Self {
        let targets = files
            .into_iter()
            .map(|file| {
                file.parent()
                    .map_or_else(|| file.clone(), Path::to_path_buf)
            })
            .collect();
        Self {
            backend,
            targets,
            watched: HashMap::new(),
            sink,
        }
    }

    pub(crate) fn refresh(&mut self) -> bool {
        let mut covered = true;
        let mut wanted = BTreeSet::new();
        for target in &self.targets {
            match target.ancestors().find(|dir| dir.is_dir()) {
                Some(dir) => {
                    wanted.insert(dir.to_path_buf());
                }
                None => covered = false,
            }
        }
        self.watched.retain(|dir, _| wanted.contains(dir));
        for dir in wanted {
            if self.watched.contains_key(&dir) {
                continue;
            }
            match self
                .backend
                .watch(&dir, WatchToken::standalone(), Arc::clone(&self.sink))
            {
                Ok(handle) => {
                    self.watched.insert(dir, handle);
                }
                Err(_) => covered = false,
            }
        }
        covered
    }
}

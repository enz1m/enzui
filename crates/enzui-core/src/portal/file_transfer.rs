//! File capabilities exchanged with sandboxed applications during a drag.
//!
//! Portal keys are capabilities: they are never displayed or logged. Exports
//! grant read access only and remain owned until the destination completes.
use ashpd::documents::file_transfer::{FileTransfer, StartTransferOptions};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// MIME used by GTK and other portal-aware file drag sources.
pub const MIME: &str = "application/vnd.portal.filetransfer";
const BUDGET: Duration = Duration::from_secs(2);
const MAX_FILES: usize = 128;

/// A bounded request failed or the document portal is unavailable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransferError;

impl std::fmt::Display for TransferError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("file transfer portal unavailable or request refused")
    }
}
impl std::error::Error for TransferError {}

/// A document-portal connection using the application's existing Tokio runtime.
#[derive(Debug)]
pub struct FileTransferClient {
    proxy: FileTransfer,
    runtime: tokio::runtime::Handle,
}

impl FileTransferClient {
    /// Connect without starting another runtime or waiting indefinitely.
    ///
    /// # Errors
    /// Returns an opaque error when no supported portal is available.
    pub async fn connect() -> Result<Arc<Self>, TransferError> {
        let connection = tokio::time::timeout(BUDGET, zbus::Connection::session())
            .await
            .map_err(|_| TransferError)?
            .map_err(|_| TransferError)?;
        Self::with_connection(connection).await
    }

    /// Use an explicit session connection, including an isolated application bus.
    ///
    /// # Errors
    /// Returns an opaque error when the connection has no document portal.
    pub async fn with_connection(connection: zbus::Connection) -> Result<Arc<Self>, TransferError> {
        let runtime = tokio::runtime::Handle::try_current().map_err(|_| TransferError)?;
        let proxy = tokio::time::timeout(BUDGET, FileTransfer::with_connection(connection))
            .await
            .map_err(|_| TransferError)?
            .map_err(|_| TransferError)?;
        Ok(Arc::new(Self { proxy, runtime }))
    }

    /// Resolve a received key to bounded, absolute local paths.
    ///
    /// # Errors
    /// Refuses malformed keys, excessive batches, relative paths and timeouts.
    pub async fn retrieve(&self, key: &[u8]) -> Result<Vec<PathBuf>, TransferError> {
        let key = std::str::from_utf8(key).map_err(|_| TransferError)?;
        if key.is_empty() || key.len() > 4096 || key.chars().any(char::is_control) {
            return Err(TransferError);
        }
        let paths =
            tokio::time::timeout(BUDGET, self.proxy.retrieve_files(key, Default::default()))
                .await
                .map_err(|_| TransferError)?
                .map_err(|_| TransferError)?;
        if paths.is_empty()
            || paths.len() > MAX_FILES
            || paths
                .iter()
                .any(|p| p.len() > 65536 || !Path::new(p).is_absolute() || p.contains('\0'))
        {
            return Err(TransferError);
        }
        Ok(paths.into_iter().map(PathBuf::from).collect())
    }

    /// Prepare regular files on a blocking worker before beginning a native drag.
    /// Directory sources retain ordinary URI interoperability instead.
    ///
    /// # Errors
    /// Refuses unreadable/nonregular files, large batches or portal failures.
    pub fn export_blocking(
        self: &Arc<Self>,
        paths: &[PathBuf],
    ) -> Result<FileExport, TransferError> {
        use std::os::unix::fs::OpenOptionsExt as _;
        if paths.is_empty() || paths.len() > MAX_FILES {
            return Err(TransferError);
        }
        let files = paths
            .iter()
            .map(|path| {
                if !path.is_absolute() {
                    return Err(TransferError);
                }
                let file = std::fs::OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_NONBLOCK)
                    .open(path)
                    .map_err(|_| TransferError)?;
                if !file.metadata().map_err(|_| TransferError)?.is_file() {
                    return Err(TransferError);
                }
                Ok(file)
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.runtime.block_on(async {
            tokio::time::timeout(BUDGET, async {
                let key = self
                    .proxy
                    .start_transfer(
                        StartTransferOptions::default()
                            .set_writeable(false)
                            .set_auto_stop(true),
                    )
                    .await
                    .map_err(|_| TransferError)?;
                let export = FileExport {
                    key,
                    client: Arc::clone(self),
                };
                for batch in files.chunks(16) {
                    self.proxy
                        .add_files(export.key(), batch, Default::default())
                        .await
                        .map_err(|_| TransferError)?;
                }
                Ok(export)
            })
            .await
            .map_err(|_| TransferError)?
        })
    }
}

/// A read-only export, retired on source cancellation or destination completion.
pub struct FileExport {
    key: String,
    client: Arc<FileTransferClient>,
}

impl FileExport {
    /// The capability sent exclusively as the portal MIME payload.
    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }
}

impl Drop for FileExport {
    fn drop(&mut self) {
        let key = std::mem::take(&mut self.key);
        let client = Arc::clone(&self.client);
        let _cleanup = self.client.runtime.spawn(async move {
            let _ = tokio::time::timeout(BUDGET, client.proxy.stop_transfer(&key)).await;
        });
    }
}

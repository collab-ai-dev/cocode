//! Cross-process serialization for rotating OAuth refresh tokens.

use std::fs::File;
use std::path::Path;

use crate::error;
use crate::error::Result;

/// Acquire a provider-scoped advisory file lock. The returned file owns the
/// lock; dropping it releases the lock. Custom/ephemeral backends pass `None`
/// and rely on the service's process-local semaphore.
pub(crate) async fn acquire(lock_dir: Option<&Path>, provider_name: &str) -> Result<Option<File>> {
    let Some(lock_dir) = lock_dir else {
        return Ok(None);
    };
    let valid_name = !provider_name.is_empty()
        && provider_name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'));
    if !valid_name {
        return Err(error::StoreSnafu {
            message: format!("invalid provider instance name '{provider_name}'"),
        }
        .build());
    }
    let lock_dir = lock_dir.to_owned();
    let provider_name = provider_name.to_owned();
    tokio::task::spawn_blocking(move || acquire_blocking(&lock_dir, &provider_name))
        .await
        .map_err(|error| {
            error::InternalSnafu {
                message: format!("join refresh-lock task: {error}"),
            }
            .build()
        })?
        .map(Some)
}

fn acquire_blocking(lock_dir: &Path, provider_name: &str) -> Result<File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(lock_dir)
            .map_err(store_error)?;
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(lock_dir).map_err(store_error)?;

    let path = lock_dir.join(format!(".{provider_name}.refresh.lock"));
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path).map_err(store_error)?;
    fs2::FileExt::lock_exclusive(&file).map_err(store_error)?;
    Ok(file)
}

fn store_error(error: std::io::Error) -> crate::error::ProviderAuthError {
    error::StoreSnafu {
        message: error.to_string(),
    }
    .build()
}

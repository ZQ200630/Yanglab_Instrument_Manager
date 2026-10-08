//! Handle-validated files and ancestor pins, adapted from the reviewed archive.
//! Paths are internal. Sharing excludes write/delete for each transaction.
use crate::WorkerError;
use std::{
    fs::{File, OpenOptions},
    io::Read,
    path::{Component, Path},
};
pub(crate) fn error(e: impl std::fmt::Display) -> WorkerError {
    WorkerError::new("CaptureStorage", e.to_string())
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Identity {
    volume: u32,
    index: u64,
}
pub(crate) fn identity(file: &File, directory: bool) -> Result<Identity, WorkerError> {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY,
            FILE_ATTRIBUTE_REPARSE_POINT,
        };
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
            return Err(error(std::io::Error::last_os_error()));
        }
        if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || (info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0) != directory
            || (!directory && info.nNumberOfLinks != 1)
        {
            return Err(error(
                "Ordinary single-link files and non-reparse directories required",
            ));
        }
        Ok(Identity {
            volume: info.dwVolumeSerialNumber,
            index: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
        })
    }
    #[cfg(not(windows))]
    {
        let _ = (file, directory);
        Err(error("Guarded capture staging requires Windows"))
    }
}
pub(crate) fn open(
    path: &Path,
    directory: bool,
    create: bool,
    delete: bool,
) -> Result<File, WorkerError> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            DELETE, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_LIST_DIRECTORY,
            FILE_READ_ATTRIBUTES, FILE_SHARE_READ,
        };
        let mut options = OpenOptions::new();
        options.share_mode(FILE_SHARE_READ).custom_flags(
            FILE_FLAG_OPEN_REPARSE_POINT
                | if directory {
                    FILE_FLAG_BACKUP_SEMANTICS
                } else {
                    0
                },
        );
        if create {
            options.write(true).create_new(true);
        } else if directory {
            options.access_mode(FILE_READ_ATTRIBUTES | FILE_LIST_DIRECTORY);
        } else if delete {
            options.access_mode(DELETE | FILE_READ_ATTRIBUTES);
        } else {
            options.read(true);
        }
        let file = options.open(path).map_err(error)?;
        identity(&file, directory)?;
        Ok(file)
    }
    #[cfg(not(windows))]
    {
        let _ = (path, directory, create, delete);
        Err(error("Guarded capture staging requires Windows"))
    }
}
/// Re-open only a partial staging file this process created and identified.
pub(crate) fn repair(path: &Path, expected: Identity) -> Result<File, WorkerError> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ};
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(error)?;
    if identity(&file, false)? != expected {
        return Err(error("Partial staging identity changed"));
    }
    Ok(file)
}
pub(crate) fn pin(path: &Path) -> Result<Vec<File>, WorkerError> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err(error("Absolute normalized staging path required"));
    }
    let mut ancestors: Vec<_> = path.ancestors().collect();
    ancestors.reverse();
    ancestors
        .into_iter()
        .map(|p| open(p, true, false, false))
        .collect()
}
pub(crate) fn read(path: &Path, expected: Identity, maximum: u64) -> Result<Vec<u8>, WorkerError> {
    let mut file = open(path, false, false, false)?;
    if identity(&file, false)? != expected {
        return Err(error("Staged file identity changed"));
    }
    let size = file.metadata().map_err(error)?.len();
    if size > maximum {
        return Err(error("Staged file exceeds bound"));
    }
    let mut bytes = Vec::with_capacity(size as usize);
    Read::by_ref(&mut file)
        .take(maximum + 1)
        .read_to_end(&mut bytes)
        .map_err(error)?;
    if bytes.len() as u64 != size {
        return Err(error("Staged file size changed"));
    }
    Ok(bytes)
}
pub(crate) fn delete(path: &Path, expected: Identity) -> Result<(), WorkerError> {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            FileDispositionInfo, SetFileInformationByHandle, FILE_DISPOSITION_INFO,
        };
        // Missing is accepted only for retrying a previously owned ACK.
        if std::fs::symlink_metadata(path).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
        {
            return Ok(());
        }
        let file = open(path, false, false, true)?;
        if identity(&file, false)? != expected {
            return Err(error("ACK file identity changed"));
        }
        let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
        if unsafe {
            SetFileInformationByHandle(
                file.as_raw_handle(),
                FileDispositionInfo,
                std::ptr::addr_of!(disposition).cast(),
                std::mem::size_of_val(&disposition) as u32,
            )
        } == 0
        {
            return Err(error(std::io::Error::last_os_error()));
        }
        drop(file);
        // Never report a promised deletion as confirmed physical capacity.
        match std::fs::symlink_metadata(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            _ => Err(error("ACK deletion not confirmed")),
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (path, expected);
        Err(error("Guarded capture staging requires Windows"))
    }
}

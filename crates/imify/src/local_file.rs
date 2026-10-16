//! Opening files of a local dataset or journal without following symbolic links.

use std::fs::{File, Metadata};
use std::io::{self, Read};
use std::path::Path;

use rustix::fs::{Mode, OFlags};
use rustix::io::Errno;

/// Opens `path` for reading when it is a regular file, and returns it with its metadata.
///
/// A symbolic link as the last component of `path` is not followed. The open does not
/// block on a FIFO or a device; either fails with [`io::ErrorKind::InvalidInput`].
pub(crate) fn open_regular(path: &Path) -> io::Result<(File, Metadata)> {
    let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
    let file = File::from(rustix::fs::open(path, flags, Mode::empty()).map_err(open_error)?);
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the path is not a regular file",
        ));
    }
    Ok((file, metadata))
}

/// Reads the regular file at `path`, as [`open_regular`] opens it, when it has at most
/// `limit` bytes. A larger file fails with [`io::ErrorKind::FileTooLarge`].
pub(crate) fn read_capped(path: &Path, limit: u64) -> io::Result<Vec<u8>> {
    let (file, metadata) = open_regular(path)?;
    let too_large = || {
        io::Error::new(
            io::ErrorKind::FileTooLarge,
            format!("the file is larger than {limit} bytes"),
        )
    };
    if metadata.len() > limit {
        return Err(too_large());
    }
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1)).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(too_large());
    }
    Ok(bytes)
}

fn open_error(errno: Errno) -> io::Error {
    // `O_NOFOLLOW` reports a symbolic link as `ELOOP`.
    if errno == Errno::LOOP {
        io::Error::new(io::ErrorKind::InvalidInput, "the path is a symbolic link")
    } else {
        errno.into()
    }
}

/// Creates a FIFO at `path`.
#[cfg(test)]
pub(crate) fn make_fifo(path: &Path) {
    let status = std::process::Command::new("mkfifo")
        .arg(path)
        .status()
        .unwrap();
    assert!(status.success(), "mkfifo {}", path.display());
}

#[cfg(test)]
mod tests;

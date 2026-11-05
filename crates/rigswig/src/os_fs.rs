//! The real file system, read-only. The only `Fs` the binary gives adapters.

use std::io;
use std::path::{Path, PathBuf};

use rigswig_core::fs::{DirEntry, EntryKind, Fs};

/// Files larger than this are refused, so a hostile repo cannot make rigswig read a huge file into memory.
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;

pub struct OsFs;

fn kind_of(ft: std::fs::FileType) -> EntryKind {
    if ft.is_symlink() {
        EntryKind::Symlink
    } else if ft.is_dir() {
        EntryKind::Dir
    } else {
        EntryKind::File
    }
}

impl Fs for OsFs {
    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        let meta = std::fs::metadata(path)?;
        if !meta.is_file() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a file"));
        }
        if meta.len() > MAX_FILE_BYTES {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "file too large to read"));
        }
        std::fs::read_to_string(path)
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
        let mut out = Vec::new();
        for e in std::fs::read_dir(path)? {
            let e = e?;
            out.push(DirEntry { name: e.file_name().to_string_lossy().into_owned(), kind: kind_of(e.file_type()?) });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    fn kind(&self, path: &Path) -> io::Result<EntryKind> {
        Ok(kind_of(std::fs::symlink_metadata(path)?.file_type()))
    }

    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        std::fs::canonicalize(path)
    }
}

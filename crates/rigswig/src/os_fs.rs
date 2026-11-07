//! The real file system, read-only. The only `Fs` the binary gives adapters.

use std::io::{self, BufRead};
use std::path::{Path, PathBuf};

use rigswig_core::fs::{DirEntry, EntryKind, Fs, MAX_LINE_BYTES};

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

    /// Streamed, with no limit on the file: memory stays at one line, and a line over the limit is skipped
    /// without being kept.
    fn for_each_line(&self, path: &Path, each: &mut dyn FnMut(Option<&str>)) -> io::Result<()> {
        if !std::fs::metadata(path)?.is_file() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a file"));
        }
        let mut r = io::BufReader::with_capacity(256 * 1024, std::fs::File::open(path)?);
        let mut line: Vec<u8> = Vec::new();
        let mut too_long = false;
        loop {
            let buf = r.fill_buf()?;
            if buf.is_empty() {
                break;
            }
            let (chunk, end) = match buf.iter().position(|&b| b == b'\n') {
                Some(i) => (&buf[..i], Some(i + 1)),
                None => (buf, None),
            };
            if !too_long {
                if line.len() + chunk.len() > MAX_LINE_BYTES {
                    too_long = true;
                    line = Vec::new();
                } else {
                    line.extend_from_slice(chunk);
                }
            }
            let used = end.unwrap_or(buf.len());
            r.consume(used);
            if end.is_some() {
                emit(&line, too_long, each);
                line.clear();
                too_long = false;
            }
        }
        if !line.is_empty() || too_long {
            emit(&line, too_long, each);
        }
        Ok(())
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

fn emit(line: &[u8], too_long: bool, each: &mut dyn FnMut(Option<&str>)) {
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    if too_long {
        each(None);
    } else if !line.iter().all(u8::is_ascii_whitespace) {
        each(std::str::from_utf8(line).ok());
    }
}

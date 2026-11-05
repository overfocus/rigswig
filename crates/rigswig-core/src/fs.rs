//! The read-only file system adapters see. The binary passes the real one; tests pass [`MemFs`].
//!
//! There is deliberately no write method: an adapter cannot change anything even by mistake.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Dir,
    Symlink,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub kind: EntryKind,
}

/// Read-only access to files. Paths are absolute.
pub trait Fs {
    fn read_to_string(&self, path: &Path) -> io::Result<String>;
    /// Entries sorted by name; symlinks are reported as symlinks, not followed.
    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>>;
    /// The kind of `path` itself, without following a final symlink. `NotFound` if absent.
    fn kind(&self, path: &Path) -> io::Result<EntryKind>;
    /// The path with every symlink resolved, so a caller can check it stays inside a root.
    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf>;

    fn exists(&self, path: &Path) -> bool {
        self.kind(path).is_ok()
    }
}

/// True if `path` (already canonical) is `root` or inside it.
pub fn is_within(path: &Path, root: &Path) -> bool {
    path.starts_with(root)
}

#[derive(Debug, Clone)]
enum Node {
    File(String),
    Dir,
    Symlink(PathBuf),
}

/// An in-memory tree for tests: files, directories (implied by their contents or added empty) and symlinks.
#[derive(Debug, Clone, Default)]
pub struct MemFs {
    nodes: BTreeMap<PathBuf, Node>,
}

const MAX_LINKS: usize = 40;

fn not_found(p: &Path) -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, format!("not found: {}", p.display()))
}

/// Lexically normalise an absolute path (drop `.`, apply `..`), without touching links.
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

impl MemFs {
    pub fn new() -> Self {
        Self::default()
    }

    fn add_parents(&mut self, p: &Path) {
        let mut cur = p.parent();
        while let Some(d) = cur {
            self.nodes.entry(d.to_path_buf()).or_insert(Node::Dir);
            cur = d.parent();
        }
    }

    /// Add a file (and its parent directories).
    pub fn file(mut self, path: impl AsRef<Path>, content: impl Into<String>) -> Self {
        let p = normalize(path.as_ref());
        self.add_parents(&p);
        self.nodes.insert(p, Node::File(content.into()));
        self
    }

    /// Add an empty directory.
    pub fn dir(mut self, path: impl AsRef<Path>) -> Self {
        let p = normalize(path.as_ref());
        self.add_parents(&p);
        self.nodes.insert(p, Node::Dir);
        self
    }

    /// Add a symlink at `path` pointing to `target` (absolute, or relative to the link's folder).
    pub fn symlink(mut self, path: impl AsRef<Path>, target: impl AsRef<Path>) -> Self {
        let p = normalize(path.as_ref());
        self.add_parents(&p);
        self.nodes.insert(p, Node::Symlink(target.as_ref().to_path_buf()));
        self
    }

    /// Resolve every symlink in `path`, component by component.
    fn resolve(&self, path: &Path) -> io::Result<PathBuf> {
        if !path.is_absolute() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "MemFs paths must be absolute"));
        }
        // Names still to walk, the next one last, so `pop` yields it. A link's target is pushed on top.
        fn push_all(stack: &mut Vec<OsString>, p: &Path) {
            let start = stack.len();
            stack.extend(p.components().filter_map(|c| match c {
                Component::Normal(s) => Some(s.to_os_string()),
                Component::ParentDir => Some(OsString::from("..")),
                _ => None,
            }));
            stack[start..].reverse();
        }
        let mut stack = Vec::new();
        push_all(&mut stack, path);

        let mut out = PathBuf::from("/");
        let mut hops = 0;
        while let Some(name) = stack.pop() {
            if name == ".." {
                out.pop();
                continue;
            }
            out.push(&name);
            match self.nodes.get(&out) {
                Some(Node::Symlink(target)) => {
                    hops += 1;
                    if hops > MAX_LINKS {
                        return Err(io::Error::other("too many levels of symbolic links"));
                    }
                    out.pop();
                    if target.is_absolute() {
                        out = PathBuf::from("/");
                    }
                    push_all(&mut stack, target);
                }
                Some(_) => {}
                None => return Err(not_found(path)),
            }
        }
        Ok(out)
    }

    fn node(&self, path: &Path) -> io::Result<&Node> {
        let real = self.resolve(path)?;
        if real.as_os_str() == "/" {
            return Ok(&Node::Dir);
        }
        self.nodes.get(&real).ok_or_else(|| not_found(path))
    }
}

impl Fs for MemFs {
    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        match self.node(path)? {
            Node::File(s) => Ok(s.clone()),
            _ => Err(io::Error::new(io::ErrorKind::InvalidInput, format!("not a file: {}", path.display()))),
        }
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
        let real = self.resolve(path)?;
        if !matches!(self.nodes.get(&real), Some(Node::Dir)) && real.as_os_str() != "/" {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("not a directory: {}", path.display())));
        }
        Ok(self
            .nodes
            .iter()
            .filter(|(p, _)| p.parent() == Some(real.as_path()))
            .map(|(p, n)| DirEntry {
                name: p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
                kind: match n {
                    Node::File(_) => EntryKind::File,
                    Node::Dir => EntryKind::Dir,
                    Node::Symlink(_) => EntryKind::Symlink,
                },
            })
            .collect())
    }

    fn kind(&self, path: &Path) -> io::Result<EntryKind> {
        let p = normalize(path);
        let parent = match p.parent() {
            Some(d) => self.resolve(d)?,
            None => return Ok(EntryKind::Dir),
        };
        let full = parent.join(p.file_name().ok_or_else(|| not_found(path))?);
        match self.nodes.get(&full) {
            Some(Node::File(_)) => Ok(EntryKind::File),
            Some(Node::Dir) => Ok(EntryKind::Dir),
            Some(Node::Symlink(_)) => Ok(EntryKind::Symlink),
            None => Err(not_found(path)),
        }
    }

    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        let real = self.resolve(path)?;
        if real.as_os_str() != "/" && !self.nodes.contains_key(&real) {
            return Err(not_found(path));
        }
        Ok(real)
    }
}

//! Auditing a repo you have not cloned: a `.tar.gz` archive, unpacked into memory, never onto disk.
//!
//! `--archive <file>` reads one you downloaded yourself (no network). `--github owner/repo[@ref]` downloads GitHub's
//! archive of a public repo first: the one network request rigswig can make, and only when a run asks for it. See
//! docs/GITHUB.md for the rules and why each exists. In short:
//!
//! - the archive becomes an in-memory tree ([`MemFs`]): nothing is written, and the tree holds nothing but the archive,
//!   so no link can reach this machine; a symbolic link is kept only when its relative target stays inside the repo
//!   (`.claude/skills -> ../.agents/skills`), every other link and special file is dropped;
//! - the harness is read with an empty, invented home folder, so the reader's own setup never mixes in;
//! - one host, `codeload.github.com`, over HTTPS; the input is checked against GitHub's own name rules and cannot be a
//!   URL to anywhere else; no redirects are followed; no token, no cookies, and a user agent with no more than
//!   `rigswig/<version>`;
//! - caps on the download, on everything it unpacks to, on the number of files and on each file.

use std::io::Read;
use std::path::{Component, Path, PathBuf};

use rigswig_core::fs::MemFs;

/// The empty home folder a fetched repo is read with.
pub const HOME: &str = "/rigswig-home";
/// Where a fetched repo sits in the in-memory tree: `/rigswig-repos/<repo>`.
pub const ROOT: &str = "/rigswig-repos";

/// Limits on one archive.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Compressed bytes read (the download, or the file).
    pub max_compressed: u64,
    /// Bytes the archive may unpack to, every entry counted, kept or not: stops an archive that expands without end.
    pub max_unpacked: u64,
    /// Entries of any kind.
    pub max_entries: usize,
    /// A larger file is kept as an empty file: its name still counts (a signal such as `Cargo.toml`), its text does not.
    pub max_file: u64,
}

pub const LIMITS: Limits = Limits {
    max_compressed: 64 * 1024 * 1024,
    max_unpacked: 512 * 1024 * 1024,
    max_entries: 50_000,
    max_file: 1024 * 1024,
};

/// What unpacking kept and left out, for the report.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Unpacked {
    pub files: usize,
    /// Symbolic links whose relative target stays inside the repo, kept as links within the in-memory tree.
    pub links_kept: usize,
    /// Every other link (absolute, leaving the repo, hard links), devices and anything else not a file or a folder.
    pub links_dropped: usize,
    /// Files over [`Limits::max_file`] or not UTF-8 text, kept by name only.
    pub kept_empty: usize,
}

/// A repo on GitHub, checked: `owner/repo` with an optional ref.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spec {
    pub owner: String,
    pub repo: String,
    /// A branch, tag or commit; `None` is the default branch.
    pub reference: Option<String>,
}

impl Spec {
    /// `owner/repo`, `owner/repo@ref`, or a github.com address (`https://github.com/owner/repo[.git][/tree/<ref>]`).
    /// Anything else is refused: this is the only text that reaches the request.
    pub fn parse(input: &str) -> Result<Self, String> {
        let s = input.trim();
        let s = s.strip_prefix("https://").or_else(|| s.strip_prefix("http://")).unwrap_or(s);
        let s = s.strip_prefix("www.").unwrap_or(s);
        let (path, from_url) = match s.strip_prefix("github.com/") {
            Some(rest) => (rest, true),
            None => (s, false),
        };
        let (path, mut reference) = match path.split_once('@') {
            Some((p, r)) if !from_url => (p, Some(r.to_string())),
            _ => (path, None),
        };
        let parts: Vec<&str> = path.trim_end_matches('/').split('/').collect();
        let (owner, repo) = match parts.as_slice() {
            [o, r] => (*o, *r),
            [o, r, "tree", rest @ ..] if from_url && !rest.is_empty() => {
                reference = Some(rest.join("/"));
                (*o, *r)
            }
            _ => return Err(format!("not a GitHub repo: {input} (use owner/repo or owner/repo@ref)")),
        };
        let repo = repo.strip_suffix(".git").unwrap_or(repo);
        let owner_ok = (1..=39).contains(&owner.len())
            && owner.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
            && !owner.starts_with('-')
            && !owner.ends_with('-');
        let repo_ok = (1..=100).contains(&repo.len())
            && repo.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
            && repo != "."
            && repo != "..";
        if !owner_ok || !repo_ok {
            return Err(format!("not a GitHub owner/repo name: {input}"));
        }
        if let Some(r) = &reference {
            let ok = (1..=200).contains(&r.len())
                && r.chars().all(|c| c.is_ascii_alphanumeric() || "._/-".contains(c))
                && !r.starts_with(['/', '-', '.'])
                && !r.ends_with('/')
                && !r.contains("..")
                && !r.contains("//");
            if !ok {
                return Err(format!("not a branch, tag or commit name rigswig accepts: {r}"));
            }
        }
        Ok(Self { owner: owner.into(), repo: repo.into(), reference })
    }

    /// The archive's address. The host is fixed; every part was checked by [`Spec::parse`].
    #[cfg_attr(not(feature = "github"), allow(dead_code))]
    pub fn url(&self) -> String {
        format!(
            "https://codeload.github.com/{}/{}/tar.gz/{}",
            self.owner,
            self.repo,
            self.reference.as_deref().unwrap_or("HEAD")
        )
    }

    pub fn label(&self) -> String {
        match &self.reference {
            Some(r) => format!("{}/{}@{r}", self.owner, self.repo),
            None => format!("{}/{}", self.owner, self.repo),
        }
    }
}

/// Unpack a `.tar.gz` into `fs` under `root`, dropping the archive's top folder (GitHub's `<repo>-<sha>/`), keeping
/// only regular files and folders, refusing any path that climbs out, and stopping at `limits`.
pub fn unpack(gz: impl Read, root: &Path, limits: Limits) -> Result<(MemFs, Unpacked), String> {
    let compressed = gz.take(limits.max_compressed + 1);
    let mut counted = CountingRead { inner: compressed, read: 0, limit: limits.max_compressed };
    let decoder = flate2::read::GzDecoder::new(&mut counted);
    let mut unpacked = UnpackLimit { inner: decoder, read: 0, limit: limits.max_unpacked };
    let mut archive = tar::Archive::new(&mut unpacked);
    let mut fs = MemFs::new().dir(root).dir(HOME);
    let mut report = Unpacked::default();
    let mut entries = 0usize;
    let iter = archive.entries().map_err(|e| format!("not a tar.gz archive: {e}"))?;
    for entry in iter {
        let mut entry = entry.map_err(|e| format!("the archive is damaged or too large: {e}"))?;
        entries += 1;
        if entries > limits.max_entries {
            return Err(format!("the archive has more than {} entries", limits.max_entries));
        }
        let kind = entry.header().entry_type();
        if matches!(kind, tar::EntryType::XGlobalHeader | tar::EntryType::XHeader) {
            continue;
        }
        let raw = entry.path().map_err(|e| format!("a path in the archive is not readable: {e}"))?.into_owned();
        let Some(rel) = inside(&raw)? else { continue };
        let dest = root.join(&rel);
        match kind {
            tar::EntryType::Directory => fs = fs.dir(&dest),
            tar::EntryType::Regular | tar::EntryType::Continuous => {
                report.files += 1;
                let size = entry.header().size().unwrap_or(u64::MAX);
                let mut text = String::new();
                if size <= limits.max_file {
                    let mut bytes = Vec::with_capacity(size as usize);
                    entry
                        .by_ref()
                        .take(limits.max_file)
                        .read_to_end(&mut bytes)
                        .map_err(|e| format!("reading the archive: {e}"))?;
                    match String::from_utf8(bytes) {
                        Ok(t) => text = t,
                        Err(_) => report.kept_empty += 1,
                    }
                } else {
                    report.kept_empty += 1;
                }
                fs = fs.file(&dest, text);
            }
            tar::EntryType::Symlink => {
                let target = entry.link_name().ok().flatten().map(|t| t.into_owned());
                match target.and_then(|t| link_inside(root, &dest, &t)) {
                    Some(t) => {
                        fs = fs.symlink(&dest, t);
                        report.links_kept += 1;
                    }
                    None => report.links_dropped += 1,
                }
            }
            _ => report.links_dropped += 1,
        }
    }
    if counted.read > limits.max_compressed {
        return Err(format!("the archive is larger than {} MB", limits.max_compressed / (1024 * 1024)));
    }
    Ok((fs, report))
}

/// Where a symbolic link at `link` points, as an absolute path in the in-memory tree, when its target is relative and,
/// resolved from the link's folder, stays inside `root`. `None` for an absolute target or one that leaves the repo. A
/// link to a link is fine: the tree resolves chains, and every hop was checked the same way.
fn link_inside(root: &Path, link: &Path, target: &Path) -> Option<PathBuf> {
    if target.as_os_str().is_empty() || target.has_root() {
        return None;
    }
    let mut out = link.parent()?.to_path_buf();
    for c in target.components() {
        match c {
            Component::Normal(p) => out.push(p),
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() || !out.starts_with(root) {
                    return None;
                }
            }
            _ => return None,
        }
    }
    (out.starts_with(root) && out != root).then_some(out)
}

/// The path below the archive's top folder, checked: `None` for the top folder itself, an error for any path that is
/// absolute or climbs with `..`.
fn inside(raw: &Path) -> Result<Option<PathBuf>, String> {
    let mut parts = Vec::new();
    for c in raw.components() {
        match c {
            Component::Normal(p) => parts.push(p.to_owned()),
            Component::CurDir => {}
            _ => return Err(format!("the archive has a path outside itself: {}", raw.display())),
        }
    }
    if parts.len() <= 1 {
        return Ok(None);
    }
    Ok(Some(parts[1..].iter().collect()))
}

struct CountingRead<R> {
    inner: R,
    read: u64,
    limit: u64,
}

impl<R: Read> Read for CountingRead<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.read += n as u64;
        if self.read > self.limit {
            return Err(std::io::Error::other("the archive is over the download limit"));
        }
        Ok(n)
    }
}

struct UnpackLimit<R> {
    inner: R,
    read: u64,
    limit: u64,
}

impl<R: Read> Read for UnpackLimit<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.read += n as u64;
        if self.read > self.limit {
            return Err(std::io::Error::other(format!(
                "the archive unpacks to more than {} MB",
                self.limit / (1024 * 1024)
            )));
        }
        Ok(n)
    }
}

/// Download `spec`'s archive. Only in a build with the `github` feature, and only called for `--github`.
/// `progress` hears the bytes received so far.
#[cfg(feature = "github")]
pub fn download(spec: &Spec, limits: Limits, progress: &mut dyn FnMut(u64)) -> Result<Vec<u8>, String> {
    let agent = ureq::AgentBuilder::new()
        .https_only(true)
        .redirects(0)
        .user_agent(&format!("rigswig/{}", rigswig_core::VERSION))
        .timeout_connect(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(180))
        .build();
    let url = spec.url();
    debug_assert!(url.starts_with("https://codeload.github.com/"));
    let resp = match agent.get(&url).call() {
        Ok(r) => r,
        Err(ureq::Error::Status(404, _)) => {
            return Err(format!(
                "{} was not found on GitHub (or it is private: rigswig fetches public repos only)",
                spec.label()
            ));
        }
        Err(ureq::Error::Status(code, _)) => return Err(format!("GitHub answered {code} for {}", spec.label())),
        Err(e) => return Err(format!("could not reach GitHub: {e}")),
    };
    if resp.status() != 200 {
        return Err(format!("GitHub answered {} for {}", resp.status(), spec.label()));
    }
    let mut body = Vec::new();
    let mut reader = resp.into_reader().take(limits.max_compressed + 1);
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut chunk).map_err(|e| format!("the download stopped: {e}"))?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
        progress(body.len() as u64);
    }
    if body.len() as u64 > limits.max_compressed {
        return Err(format!("the archive is larger than {} MB", limits.max_compressed / (1024 * 1024)));
    }
    Ok(body)
}

#[cfg(not(feature = "github"))]
pub fn download(_: &Spec, _: Limits, _: &mut dyn FnMut(u64)) -> Result<Vec<u8>, String> {
    Err("this rigswig was built without the github feature, so it makes no network requests; download the archive yourself and use --archive".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_accept_github_names_only() {
        let s = Spec::parse("anthropics/claude-code").unwrap();
        assert_eq!((s.owner.as_str(), s.repo.as_str(), s.reference.as_deref()), ("anthropics", "claude-code", None));
        assert_eq!(s.url(), "https://codeload.github.com/anthropics/claude-code/tar.gz/HEAD");
        let r = Spec::parse("o/r@v1.2.3").unwrap();
        assert_eq!(r.url(), "https://codeload.github.com/o/r/tar.gz/v1.2.3");
        let u = Spec::parse("https://github.com/o/r.git").unwrap();
        assert_eq!((u.repo.as_str(), u.reference), ("r", None));
        let t = Spec::parse("https://github.com/o/r/tree/feat/x").unwrap();
        assert_eq!(t.reference.as_deref(), Some("feat/x"));
        assert_eq!(t.label(), "o/r@feat/x");

        for bad in [
            "https://evil.example/o/r",
            "file:///etc/passwd",
            "o",
            "o/r/x",
            "-o/r",
            "o/..",
            "o/r@../../x",
            "o/r@-x",
            "o/r@a b",
            "o/r@x?y=1",
            "o/r@x#y",
            "o@x/r",
            "codeload.github.com/o/r",
            "github.com.evil.example/o/r",
            "o/r\u{0}",
        ] {
            assert!(Spec::parse(bad).is_err(), "{bad} must be refused");
        }
    }

    /// A tar.gz built entry by entry, with raw names (so `..` and absolute paths can be written).
    fn tarball(entries: &[(&str, tar::EntryType, &[u8], Option<&str>)]) -> Vec<u8> {
        let mut b = tar::Builder::new(Vec::new());
        for (name, kind, data, link) in entries {
            let mut h = tar::Header::new_gnu();
            {
                let raw = &mut h.as_old_mut().name;
                raw[..name.len()].copy_from_slice(name.as_bytes());
            }
            h.set_entry_type(*kind);
            h.set_size(data.len() as u64);
            h.set_mode(0o644);
            if let Some(l) = link {
                h.set_link_name(l).unwrap();
            }
            h.set_cksum();
            b.append(&h, *data).unwrap();
        }
        let tar = b.into_inner().unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        std::io::Write::write_all(&mut gz, &tar).unwrap();
        gz.finish().unwrap()
    }

    fn root() -> PathBuf {
        PathBuf::from(ROOT).join("r")
    }

    #[test]
    fn unpacks_files_into_memory_and_drops_every_link() {
        use rigswig_core::fs::Fs;
        let gz = tarball(&[
            ("r-abc/", tar::EntryType::Directory, b"", None),
            ("r-abc/CLAUDE.md", tar::EntryType::Regular, b"# hi\n", None),
            ("r-abc/docs/a.md", tar::EntryType::Regular, b"a", None),
            ("r-abc/AGENTS.md", tar::EntryType::Symlink, b"", Some("/etc/passwd")),
            ("r-abc/hard.md", tar::EntryType::Link, b"", Some("r-abc/CLAUDE.md")),
            ("r-abc/fifo", tar::EntryType::Fifo, b"", None),
            ("r-abc/logo.png", tar::EntryType::Regular, &[0xff, 0xfe, 0x00], None),
        ]);
        let (fs, rep) = unpack(&gz[..], &root(), LIMITS).unwrap();
        assert_eq!(fs.read_to_string(&root().join("CLAUDE.md")).unwrap(), "# hi\n");
        assert!(fs.exists(&root().join("docs/a.md")));
        assert!(!fs.exists(&root().join("AGENTS.md")), "a symlink is dropped, not followed");
        assert!(!fs.exists(&root().join("hard.md")), "a hard link is dropped");
        assert_eq!(fs.read_to_string(&root().join("logo.png")).unwrap(), "", "binary files are kept by name only");
        assert_eq!(rep, Unpacked { files: 3, links_kept: 0, links_dropped: 3, kept_empty: 1 });
        assert!(fs.exists(Path::new(HOME)));
    }

    #[test]
    fn keeps_links_that_stay_inside_the_repo_and_drops_the_rest() {
        use rigswig_core::fs::Fs;
        let gz = tarball(&[
            ("r-abc/.agents/skills/guide/SKILL.md", tar::EntryType::Regular, b"---\ndescription: guide\n---\n", None),
            ("r-abc/.claude/skills/guide", tar::EntryType::Symlink, b"", Some("../../.agents/skills/guide")),
            ("r-abc/docs/rules.md", tar::EntryType::Regular, b"rules", None),
            ("r-abc/CLAUDE.md", tar::EntryType::Symlink, b"", Some("docs/rules.md")),
            ("r-abc/chain.md", tar::EntryType::Symlink, b"", Some("CLAUDE.md")),
            ("r-abc/up.md", tar::EntryType::Symlink, b"", Some("../outside.md")),
            ("r-abc/deep/up.md", tar::EntryType::Symlink, b"", Some("../../../../etc/passwd")),
            ("r-abc/abs.md", tar::EntryType::Symlink, b"", Some("/etc/passwd")),
            ("r-abc/root", tar::EntryType::Symlink, b"", Some("..")),
        ]);
        let (fs, rep) = unpack(&gz[..], &root(), LIMITS).unwrap();
        let r = root();
        assert!(fs.read_to_string(&r.join(".claude/skills/guide/SKILL.md")).unwrap().contains("guide"));
        assert_eq!(fs.read_to_string(&r.join("CLAUDE.md")).unwrap(), "rules");
        assert_eq!(fs.read_to_string(&r.join("chain.md")).unwrap(), "rules", "a chain of inside links resolves");
        for gone in ["up.md", "deep/up.md", "abs.md", "root"] {
            assert!(!fs.exists(&r.join(gone)), "{gone} leaves the repo and is dropped");
        }
        assert_eq!((rep.links_kept, rep.links_dropped), (3, 4));
    }

    #[test]
    fn refuses_paths_that_climb_out_or_start_at_the_root() {
        for name in ["r-abc/../../etc/x", "/etc/x", "r-abc/a/../../../x"] {
            let gz = tarball(&[(name, tar::EntryType::Regular, b"x", None)]);
            assert!(unpack(&gz[..], &root(), LIMITS).is_err(), "{name} must be refused");
        }
    }

    #[test]
    fn stops_at_every_limit() {
        let big = vec![b'a'; 4096];
        let gz = tarball(&[
            ("r/a.md", tar::EntryType::Regular, &big, None),
            ("r/b.md", tar::EntryType::Regular, &big, None),
        ]);
        let small = |f: fn(&mut Limits)| {
            let mut l = LIMITS;
            f(&mut l);
            l
        };
        let e = unpack(&gz[..], &root(), small(|l| l.max_unpacked = 4096)).unwrap_err();
        assert!(e.contains("unpacks to more than") || e.contains("damaged or too large"), "{e}");
        assert!(unpack(&gz[..], &root(), small(|l| l.max_entries = 1)).unwrap_err().contains("more than 1 entries"));
        assert!(unpack(&gz[..], &root(), small(|l| l.max_compressed = 10)).is_err());
        let (fs, rep) = unpack(&gz[..], &root(), small(|l| l.max_file = 100)).unwrap();
        use rigswig_core::fs::Fs;
        assert_eq!(fs.read_to_string(&root().join("a.md")).unwrap(), "", "an oversized file is kept by name only");
        assert_eq!(rep.kept_empty, 2);
        assert!(unpack(&b"not gzip at all"[..], &root(), LIMITS).is_err());
    }
}

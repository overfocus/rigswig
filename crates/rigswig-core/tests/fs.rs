use std::path::{Path, PathBuf};

use rigswig_core::fs::{EntryKind, Fs, MemFs, is_within};

fn tree() -> MemFs {
    MemFs::new()
        .file("/home/u/.tool/settings.json", "{}")
        .file("/home/u/.tool/agents/b.md", "b")
        .file("/home/u/.tool/agents/a.md", "a")
        .dir("/home/u/.tool/empty")
        .file("/secret/key.txt", "s3cret")
        .symlink("/home/u/.tool/agents/escape.md", "/secret/key.txt")
        .symlink("/home/u/.tool/agents/sibling.md", "a.md")
        .symlink("/home/u/.tool/up", "../.tool/agents")
        .symlink("/loop/a", "/loop/b")
        .symlink("/loop/b", "/loop/a")
}

#[test]
fn reads_files_and_lists_dirs_sorted_without_following_links() {
    let fs = tree();
    assert_eq!(fs.read_to_string(Path::new("/home/u/.tool/settings.json")).unwrap(), "{}");
    let names: Vec<(String, EntryKind)> =
        fs.read_dir(Path::new("/home/u/.tool/agents")).unwrap().into_iter().map(|e| (e.name, e.kind)).collect();
    assert_eq!(
        names,
        vec![
            ("a.md".into(), EntryKind::File),
            ("b.md".into(), EntryKind::File),
            ("escape.md".into(), EntryKind::Symlink),
            ("sibling.md".into(), EntryKind::Symlink),
        ]
    );
    assert!(fs.read_dir(Path::new("/home/u/.tool/empty")).unwrap().is_empty());
}

#[test]
fn kind_does_not_follow_the_final_link() {
    let fs = tree();
    assert_eq!(fs.kind(Path::new("/home/u/.tool/agents/escape.md")).unwrap(), EntryKind::Symlink);
    assert_eq!(fs.kind(Path::new("/home/u/.tool/empty")).unwrap(), EntryKind::Dir);
    assert!(!fs.exists(Path::new("/home/u/.tool/nope")));
}

#[test]
fn canonicalize_resolves_links_so_escapes_can_be_caught() {
    let fs = tree();
    let root = Path::new("/home/u/.tool");
    let escape = fs.canonicalize(Path::new("/home/u/.tool/agents/escape.md")).unwrap();
    assert_eq!(escape, PathBuf::from("/secret/key.txt"));
    assert!(!is_within(&escape, root));

    let sibling = fs.canonicalize(Path::new("/home/u/.tool/agents/sibling.md")).unwrap();
    assert_eq!(sibling, PathBuf::from("/home/u/.tool/agents/a.md"));
    assert!(is_within(&sibling, root));

    // A relative link with `..`, then a path through it.
    assert_eq!(
        fs.canonicalize(Path::new("/home/u/.tool/up/b.md")).unwrap(),
        PathBuf::from("/home/u/.tool/agents/b.md")
    );
    assert_eq!(fs.read_to_string(Path::new("/home/u/.tool/up/b.md")).unwrap(), "b");
}

#[test]
fn missing_paths_link_loops_and_relative_paths_are_errors() {
    let fs = tree();
    assert!(fs.read_to_string(Path::new("/home/u/.tool/missing.json")).is_err());
    assert!(fs.canonicalize(Path::new("/loop/a")).is_err());
    assert!(fs.read_to_string(Path::new("relative.json")).is_err());
    assert!(fs.read_to_string(Path::new("/home/u/.tool")).is_err(), "a directory is not a file");
}

#[test]
fn dot_dot_cannot_climb_above_root() {
    let fs = tree();
    assert_eq!(fs.canonicalize(Path::new("/../../secret/key.txt")).unwrap(), PathBuf::from("/secret/key.txt"));
}

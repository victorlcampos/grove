//! Measures how much disk a folder takes, like `du`: allocated blocks, hard links counted
//! once, symlinks not followed, other file systems and nested worktrees left out.

use std::collections::HashSet;
use std::fs::{self, Metadata};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use rayon::prelude::*;

/// Running totals of a measurement, read by the interface while it goes.
#[derive(Debug, Default)]
pub struct Progress {
    pub bytes: AtomicU64,
    pub files: AtomicU64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub bytes: u64,
    pub files: u64,
    /// The entries right under the folder, largest first.
    pub top: Vec<(String, u64)>,
}

struct Walk<'a> {
    device: Option<u64>,
    exclude: &'a HashSet<PathBuf>,
    cancel: &'a AtomicBool,
    progress: &'a Progress,
    /// Files with several hard links already counted, by device and inode (a Unix idea).
    #[cfg_attr(not(unix), allow(dead_code))]
    links: Mutex<HashSet<(u64, u64)>>,
}

/// Measures `root`, leaving out the folders in `exclude`. Runs in the current rayon pool.
/// Returns `None` when cancelled or when `root` is not a readable folder.
pub fn measure(
    root: &Path,
    exclude: &HashSet<PathBuf>,
    cancel: &AtomicBool,
    progress: &Progress,
) -> Option<Usage> {
    let meta = fs::symlink_metadata(root).ok()?;
    if !meta.is_dir() {
        return None;
    }
    let walk = Walk {
        device: device(&meta),
        exclude,
        cancel,
        progress,
        links: Mutex::default(),
    };
    let entries: Vec<fs::DirEntry> = fs::read_dir(root).ok()?.flatten().collect();
    let mut top: Vec<(String, u64)> = entries
        .par_iter()
        .filter_map(|entry| {
            let meta = entry.metadata().ok()?;
            let bytes = if meta.is_dir() {
                let path = entry.path();
                if walk.skip(&path, &meta) {
                    return None;
                }
                walk.count(allocated(&meta), 0);
                allocated(&meta) + walk.dir(&path)
            } else if walk.first_link(&meta) {
                walk.count(allocated(&meta), 1);
                allocated(&meta)
            } else {
                return None;
            };
            Some((entry.file_name().to_string_lossy().into_owned(), bytes))
        })
        .collect();
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    top.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    Some(Usage {
        bytes: allocated(&meta) + top.iter().map(|(_, bytes)| bytes).sum::<u64>(),
        files: progress.files.load(Ordering::Relaxed),
        top,
    })
}

impl Walk<'_> {
    fn dir(&self, dir: &Path) -> u64 {
        if self.cancel.load(Ordering::Relaxed) {
            return 0;
        }
        let Ok(entries) = fs::read_dir(dir) else {
            return 0;
        };
        let mut bytes = 0;
        let mut files = 0;
        let mut subdirs = Vec::new();
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                let path = entry.path();
                if !self.skip(&path, &meta) {
                    bytes += allocated(&meta);
                    subdirs.push(path);
                }
            } else if self.first_link(&meta) {
                bytes += allocated(&meta);
                files += 1;
            }
        }
        self.count(bytes, files);
        bytes
            + subdirs
                .par_iter()
                .map(|subdir| self.dir(subdir))
                .sum::<u64>()
    }

    fn count(&self, bytes: u64, files: u64) {
        self.progress.bytes.fetch_add(bytes, Ordering::Relaxed);
        self.progress.files.fetch_add(files, Ordering::Relaxed);
    }

    fn skip(&self, path: &Path, meta: &Metadata) -> bool {
        self.exclude.contains(path) || device(meta) != self.device
    }

    /// A file with several hard links counts only the first time it shows up.
    fn first_link(&self, meta: &Metadata) -> bool {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if meta.nlink() > 1 {
                let mut links = self.links.lock().unwrap_or_else(|e| e.into_inner());
                return links.insert((meta.dev(), meta.ino()));
            }
        }
        let _ = meta;
        true
    }
}

#[cfg(unix)]
fn allocated(meta: &Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    meta.blocks() * 512
}

#[cfg(not(unix))]
fn allocated(meta: &Metadata) -> u64 {
    meta.len()
}

#[cfg(unix)]
fn device(meta: &Metadata) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    Some(meta.dev())
}

#[cfg(not(unix))]
fn device(_: &Metadata) -> Option<u64> {
    None
}

/// Free and total space of the volume holding a folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Volume {
    pub free: u64,
    pub total: u64,
}

#[cfg(unix)]
#[allow(clippy::unnecessary_cast)] // the field types differ between systems
pub fn volume(path: &Path) -> Option<Volume> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let c_path = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `c_path` is a valid C string and `stat` has room for one `statvfs`.
    if unsafe { libc::statvfs(c_path.as_ptr(), stat.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: `statvfs` succeeded, so it filled `stat`.
    let stat = unsafe { stat.assume_init() };
    let unit = stat.f_frsize as u64;
    Some(Volume {
        free: stat.f_bavail as u64 * unit,
        total: stat.f_blocks as u64 * unit,
    })
}

#[cfg(not(unix))]
pub fn volume(_: &Path) -> Option<Volume> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::tests::Scratch;

    fn run(root: &Path, exclude: &[PathBuf]) -> Usage {
        let exclude: HashSet<PathBuf> = exclude.iter().cloned().collect();
        measure(
            root,
            &exclude,
            &AtomicBool::new(false),
            &Progress::default(),
        )
        .unwrap()
    }

    #[test]
    fn adds_up_every_file_once() {
        let scratch = Scratch::new("du");
        let root = &scratch.0;
        fs::create_dir_all(root.join("big/deeper")).unwrap();
        fs::create_dir_all(root.join("small")).unwrap();
        fs::write(root.join("big/deeper/a.bin"), vec![1u8; 300_000]).unwrap();
        fs::write(root.join("big/b.bin"), vec![1u8; 100_000]).unwrap();
        fs::write(root.join("small/c.txt"), b"hello").unwrap();
        fs::write(root.join("README"), b"hi").unwrap();

        let usage = run(root, &[]);
        assert_eq!(usage.files, 4);
        assert!(usage.bytes >= 400_000, "{usage:?}");
        let names: Vec<&str> = usage.top.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names[0], "big");
        assert!(usage.top[0].1 >= 400_000);
        assert_eq!(usage.top.len(), 3);
        assert_eq!(
            usage.bytes,
            allocated(&fs::metadata(root).unwrap()) + usage.top.iter().map(|t| t.1).sum::<u64>()
        );
    }

    #[test]
    fn leaves_out_nested_worktrees() {
        let scratch = Scratch::new("du-nested");
        let root = &scratch.0;
        let nested = root.join(".claude/worktrees/x");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("huge.bin"), vec![1u8; 500_000]).unwrap();
        fs::write(root.join("mine.txt"), b"mine").unwrap();
        let usage = run(root, std::slice::from_ref(&nested));
        assert_eq!(usage.files, 1);
        assert!(usage.bytes < 100_000, "{usage:?}");
    }

    #[cfg(unix)]
    #[test]
    fn counts_hard_links_once_and_does_not_follow_symlinks() {
        let scratch = Scratch::new("du-links");
        let root = &scratch.0;
        let outside = Scratch::new("du-outside");
        fs::write(outside.0.join("elsewhere.bin"), vec![1u8; 800_000]).unwrap();
        std::os::unix::fs::symlink(&outside.0, root.join("link")).unwrap();
        fs::write(root.join("a.bin"), vec![1u8; 200_000]).unwrap();
        fs::hard_link(root.join("a.bin"), root.join("b.bin")).unwrap();
        let usage = run(root, &[]);
        assert_eq!(usage.files, 2, "the file and the symlink");
        assert!(usage.bytes >= 200_000 && usage.bytes < 400_000, "{usage:?}");
    }

    #[test]
    fn stops_when_cancelled() {
        let scratch = Scratch::new("du-cancel");
        fs::write(scratch.0.join("a"), b"a").unwrap();
        let cancelled = AtomicBool::new(true);
        assert_eq!(
            measure(
                &scratch.0,
                &HashSet::new(),
                &cancelled,
                &Progress::default()
            ),
            None
        );
        assert_eq!(
            measure(
                &scratch.0.join("a"),
                &HashSet::new(),
                &AtomicBool::new(false),
                &Progress::default()
            ),
            None,
            "a file is not a folder"
        );
    }

    #[cfg(unix)]
    #[test]
    fn reads_the_volume_space() {
        let volume = volume(Path::new("/")).unwrap();
        assert!(volume.total > 0 && volume.free <= volume.total);
    }
}

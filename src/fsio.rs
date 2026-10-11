//! Bounded reads of project and user files. Only regular files are opened for reading, so a FIFO,
//! device or directory with a manifest's name cannot hang or flood the completion worker.
use std::{fs, io::Read, os::unix::fs::OpenOptionsExt, path::Path};
#[cfg(target_os = "macos")]
const O_NONBLOCK: i32 = 0x0004;
#[cfg(not(target_os = "macos"))]
const O_NONBLOCK: i32 = 0o4000;
/// Entries read from one directory before sorting; larger directories yield an arbitrary subset.
pub const MAX_DIR_ENTRIES: usize = 50_000;
/// At most `max` bytes of a regular file as text; invalid UTF-8 (including a cut multi-byte
/// character at the limit) becomes U+FFFD. Opened non-blocking and checked with `fstat` on the
/// handle itself, so replacing the path between check and open cannot cause a hang.
pub fn read_text(path: &Path, max: u64) -> Option<String> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(O_NONBLOCK)
        .open(path)
        .ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = vec![];
    file.take(max).read_to_end(&mut bytes).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}
pub struct Entry {
    pub name: String,
    pub file_type: fs::FileType,
}
/// UTF-8 named entries of a directory sorted by name; `None` when it cannot be listed. Symlinks keep
/// their own file type, so callers that follow them stat the path themselves.
pub fn list_dir(dir: &Path, max: usize) -> Option<Vec<Entry>> {
    let mut out: Vec<Entry> = fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|e| {
            Some(Entry {
                name: e.file_name().into_string().ok()?,
                file_type: e.file_type().ok()?,
            })
        })
        .take(max)
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Some(out)
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::{process::Command, sync::mpsc, time::Duration};
    #[test]
    fn non_regular_files_are_ignored_without_blocking() {
        let dir = std::env::temp_dir().join(format!("riga-fsio-{}", std::process::id()));
        fs::create_dir_all(dir.join("adir")).unwrap();
        fs::write(dir.join("big"), "é".repeat(10)).unwrap();
        let fifo = dir.join("fifo");
        assert!(
            Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );
        let (tx, rx) = mpsc::channel();
        let (f, d, b) = (fifo.clone(), dir.join("adir"), dir.join("big"));
        std::thread::spawn(move || {
            // Seven bytes cut the fourth "é" in half.
            let _ = tx.send((read_text(&f, 99), read_text(&d, 99), read_text(&b, 7)));
        });
        let (fifo_text, dir_text, cut) = rx.recv_timeout(Duration::from_secs(5)).expect("hung");
        assert_eq!((fifo_text, dir_text), (None, None));
        assert_eq!(cut.as_deref(), Some("ééé\u{fffd}"));
        let names: Vec<_> = list_dir(&dir, 10)
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, ["adir", "big", "fifo"]);
        assert!(list_dir(&dir.join("missing"), 10).is_none());
        fs::remove_dir_all(dir).unwrap();
    }
}

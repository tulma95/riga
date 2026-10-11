//! Fixed, read-only tool runs (`tmux list-sessions`, `ffmpeg -encoders`, ...) in background threads.
//!
//! Invariants: `get` never waits and never spawns more than one run per key; the cache holds at
//! most `MAX_ENTRIES` keys (least recently used idle entry is evicted); a failed run keeps the
//! previous good lines and is retried only after `RETRY`; every child runs in its own process
//! group which is killed and reaped when the run ends, so no descendant outlives it.
use std::{
    collections::HashMap,
    io::{Read, Write},
    os::unix::process::CommandExt,
    path::PathBuf,
    process::{Command, Stdio},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};
const TIMEOUT: Duration = Duration::from_secs(3);
const RETRY: Duration = Duration::from_secs(15);
const MAX_BYTES: u64 = 1 << 20;
const MAX_LINES: usize = 5_000;
const MAX_ENTRIES: usize = 64;
unsafe extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}
/// (shell PATH, argv): the same command under another PATH may resolve to another tool.
pub type Key = (String, Vec<String>);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Within its TTL; `lines` are current.
    Fresh,
    /// A run is in flight; `lines` are the previous result (possibly empty) and a notification
    /// for this key follows.
    Pending,
    /// The last run failed (or the cache is full of running entries); `lines` are the last good
    /// result, if any. No notification will follow until a retry is started by a later `get`.
    Failed,
}
pub struct Snapshot {
    pub lines: Vec<String>,
    pub state: State,
}
#[derive(Default)]
struct Entry {
    lines: Vec<String>,
    fetched: Option<Instant>,
    failed: Option<Instant>,
    running: bool,
    used: u64,
}
#[derive(Default)]
struct Cache {
    entries: HashMap<Key, Entry>,
    tick: u64,
    /// Keys whose `get` returned `Pending` since the last `take_wanted`.
    wanted: Vec<Key>,
}
/// Fetch function: resolved through the shell PATH, returns lines or `None` on any failure.
type Exec = Arc<dyn Fn(&str, &[String]) -> Option<Vec<String>> + Send + Sync>;
type Notify = Arc<dyn Fn(&Key) + Send + Sync>;
pub struct Runs {
    cache: Arc<Mutex<Cache>>,
    notify: Option<Notify>,
    exec: Exec,
    retry: Duration,
}
impl Default for Runs {
    fn default() -> Self {
        Self::with_exec(|path, argv| run(path, argv, None, TIMEOUT))
    }
}
impl Runs {
    /// Like `default`, except that runs `custom` recognizes (`Some`) are answered by it instead of
    /// starting a tool.
    pub fn with_custom(
        custom: impl Fn(&str, &[String]) -> Option<Option<Vec<String>>> + Send + Sync + 'static,
    ) -> Self {
        Self::with_exec(move |path, argv| {
            custom(path, argv).unwrap_or_else(|| run(path, argv, None, TIMEOUT))
        })
    }
    pub fn with_exec(
        exec: impl Fn(&str, &[String]) -> Option<Vec<String>> + Send + Sync + 'static,
    ) -> Self {
        Self {
            cache: Arc::default(),
            notify: None,
            exec: Arc::new(exec),
            retry: RETRY,
        }
    }
    /// Called from the fetch thread after every finished run (changed or not), once the entry is
    /// no longer `running`.
    pub fn on_update(&mut self, f: impl Fn(&Key) + Send + Sync + 'static) {
        self.notify = Some(Arc::new(f));
    }
    pub fn busy(&self) -> bool {
        self.cache
            .lock()
            .unwrap()
            .entries
            .values()
            .any(|e| e.running)
    }
    pub fn is_running(&self, key: &Key) -> bool {
        self.cache
            .lock()
            .unwrap()
            .entries
            .get(key)
            .is_some_and(|e| e.running)
    }
    /// Keys that were `Pending` when looked up since the last call.
    pub fn take_wanted(&self) -> Vec<Key> {
        std::mem::take(&mut self.cache.lock().unwrap().wanted)
    }
    /// Cached stdout lines of `argv` resolved through `path`.
    pub fn lines(&self, path: &str, argv: &[&str], ttl: Duration) -> Vec<String> {
        self.get(path, argv, ttl).lines
    }
    /// Cached result for `argv`; starts at most one refresh per key when stale. Never waits.
    pub fn get(&self, path: &str, argv: &[&str], ttl: Duration) -> Snapshot {
        let key: Key = (path.into(), argv.iter().map(|&a| a.into()).collect());
        let mut cache = self.cache.lock().unwrap();
        cache.tick += 1;
        let tick = cache.tick;
        if !cache.entries.contains_key(&key) && cache.entries.len() >= MAX_ENTRIES {
            let lru = cache
                .entries
                .iter()
                .filter(|(_, e)| !e.running)
                .min_by_key(|(_, e)| e.used)
                .map(|(k, _)| k.clone());
            match lru {
                Some(k) => {
                    cache.entries.remove(&k);
                }
                None => {
                    return Snapshot {
                        lines: vec![],
                        state: State::Failed,
                    };
                }
            }
        }
        let entry = cache.entries.entry(key.clone()).or_default();
        entry.used = tick;
        let due = !entry.running
            && entry.fetched.is_none_or(|t| t.elapsed() >= ttl)
            && entry.failed.is_none_or(|t| t.elapsed() >= self.retry);
        if due {
            entry.running = true;
            let (shared, notify, exec) =
                (self.cache.clone(), self.notify.clone(), self.exec.clone());
            let k = key.clone();
            let spawned = thread::Builder::new().spawn(move || {
                let result = exec(&k.0, &k.1);
                if let Some(entry) = shared.lock().unwrap().entries.get_mut(&k) {
                    entry.running = false;
                    match result {
                        Some(lines) => {
                            (entry.lines, entry.fetched, entry.failed) =
                                (lines, Some(Instant::now()), None);
                        }
                        None => entry.failed = Some(Instant::now()),
                    }
                }
                if let Some(notify) = notify {
                    notify(&k);
                }
            });
            if spawned.is_err() {
                entry.running = false;
                entry.failed = Some(Instant::now());
            }
        }
        let state = if entry.running {
            State::Pending
        } else if entry.failed.is_some() {
            State::Failed
        } else {
            State::Fresh
        };
        let lines = entry.lines.clone();
        if state == State::Pending && !cache.wanted.contains(&key) {
            cache.wanted.push(key);
        }
        Snapshot { lines, state }
    }
}
/// First executable named `program` in the shell's PATH (absolute entries only).
fn resolve(path: &str, program: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    std::env::split_paths(path)
        .filter(|d| d.is_absolute())
        .map(|d| d.join(program))
        .find(|p| {
            std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}
/// SIGKILL to every process in the group `pgid` (the child and all its descendants).
fn kill_group(pgid: u32) {
    unsafe {
        kill(-(pgid as i32), 9);
    }
}
/// Output of a successful run within `timeout`; `None` if missing, failing, slow or oversized.
/// `input` is written to stdin (private data never goes in argv, which `ps` shows).
/// The child leads its own process group, which is always killed and the child reaped before
/// returning; the reader thread ends once the group is dead (its pipe closes).
pub(crate) fn run(
    path: &str,
    argv: &[String],
    input: Option<&str>,
    timeout: Duration,
) -> Option<Vec<String>> {
    let (program, args) = argv.split_first()?;
    let mut child = Command::new(resolve(path, program)?)
        .args(args)
        .env("PATH", path)
        .process_group(0)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let pgid = child.id();
    // A writer thread, so a child that never reads cannot stall us past the timeout.
    if let (Some(mut stdin), Some(input)) = (child.stdin.take(), input) {
        let input = input.to_owned();
        thread::spawn(move || stdin.write_all(input.as_bytes()));
    }
    let Some(stdout) = child.stdout.take() else {
        kill_group(pgid);
        let _ = child.wait();
        return None;
    };
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut out = vec![];
        let ok = stdout.take(MAX_BYTES + 1).read_to_end(&mut out).is_ok();
        let _ = tx.send((ok && out.len() as u64 <= MAX_BYTES).then_some(out));
    });
    let deadline = Instant::now() + timeout;
    let out = rx.recv_timeout(timeout).ok().flatten();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if out.is_some() && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(10))
            }
            _ => break None,
        }
    };
    // The child is unreaped (or just reaped by us above), so its group id cannot be reused yet.
    kill_group(pgid);
    if status.is_none() {
        let _ = child.wait();
    }
    if !status?.success() {
        return None;
    }
    // Tabs separate fields; any other control or bidi character drops the line.
    let text = String::from_utf8_lossy(&out?).into_owned();
    Some(
        text.lines()
            .filter(|l| !l.is_empty() && !l.chars().any(|c| c != '\t' && crate::spec::deceptive(c)))
            .take(MAX_LINES)
            .map(str::to_owned)
            .collect(),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt, process::Command as Cmd, sync::atomic::*};
    const WAIT: Duration = Duration::from_secs(30);
    fn wait_idle(runs: &Runs) {
        let start = Instant::now();
        while runs.busy() {
            assert!(start.elapsed() < WAIT, "background run never finished");
            thread::sleep(Duration::from_millis(5));
        }
    }
    fn tool_dir(name: &str, script: &str) -> (PathBuf, String) {
        let root = std::env::temp_dir().join(format!("riga-bg-{name}-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let tool = root.join("fake-tool");
        fs::write(&tool, script).unwrap();
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
        let path = root.to_string_lossy().into_owned();
        (root, path)
    }
    #[test]
    fn get_never_blocks_and_runs_once_per_key() {
        let (gate_tx, gate_rx) = mpsc::channel::<()>();
        let gate_rx = Mutex::new(gate_rx);
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let mut runs = Runs::with_exec(move |_, _| {
            c.fetch_add(1, Ordering::SeqCst);
            gate_rx.lock().unwrap().recv().ok()?;
            Some(vec!["one".into()])
        });
        let (tx, rx) = mpsc::channel();
        runs.on_update(move |k| tx.send(k.clone()).unwrap());
        let ttl = Duration::from_secs(60);
        // The exec is blocked until we release it, so returning at all proves `get` did not wait.
        for _ in 0..2 {
            let s = runs.get("/p", &["t"], ttl);
            assert!(s.lines.is_empty() && s.state == State::Pending);
        }
        assert!(runs.busy());
        assert_eq!(runs.take_wanted().len(), 1);
        gate_tx.send(()).unwrap();
        let key = rx.recv_timeout(WAIT).expect("no completion notification");
        assert_eq!(key, ("/p".to_string(), vec!["t".to_string()]));
        assert!(!runs.is_running(&key));
        let s = runs.get("/p", &["t"], ttl);
        assert_eq!((s.lines, s.state), (vec!["one".to_string()], State::Fresh));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn failure_keeps_good_lines_and_backs_off() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let mut runs = Runs::with_exec(move |_, _| {
            (c.fetch_add(1, Ordering::SeqCst) == 0).then(|| vec!["good".to_string()])
        });
        let (tx, rx) = mpsc::channel();
        runs.on_update(move |_| tx.send(()).unwrap());
        let zero = Duration::ZERO;
        runs.get("/p", &["t"], zero);
        rx.recv_timeout(WAIT).unwrap();
        runs.get("/p", &["t"], zero); // stale at ttl 0: second run fails
        rx.recv_timeout(WAIT).unwrap();
        let s = runs.get("/p", &["t"], zero);
        assert_eq!(
            (s.lines.clone(), s.state),
            (vec!["good".to_string()], State::Failed)
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "retried inside the backoff"
        );
        runs.retry = Duration::ZERO;
        runs.get("/p", &["t"], zero);
        rx.recv_timeout(WAIT).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert_eq!(
            runs.get("/p", &["t"], Duration::from_secs(60)).lines,
            ["good"]
        );
    }
    #[test]
    fn eviction_drops_one_least_recently_used_entry() {
        let runs = Runs::with_exec(|_, a| Some(a.to_vec()));
        let ttl = Duration::from_secs(60);
        for i in 0..MAX_ENTRIES {
            runs.get("/p", &[&format!("k{i}")], ttl);
            wait_idle(&runs);
        }
        runs.get("/p", &["k0"], ttl); // k0 is now the most recently used
        runs.get("/p", &["new"], ttl);
        wait_idle(&runs);
        let cache = runs.cache.lock().unwrap();
        let has = |n: &str| {
            cache
                .entries
                .contains_key(&("/p".to_string(), vec![n.to_string()]))
        };
        assert_eq!(cache.entries.len(), MAX_ENTRIES);
        assert!(has("k0") && has("new") && has("k5") && !has("k1"));
    }
    #[test]
    fn real_runs_parse_output_and_reject_failures() {
        let (root, path) = tool_dir(
            "real",
            "#!/bin/sh\nprintf 'one\\ttab\\n\\033bad\\ntwo\\n'\n",
        );
        let t = Duration::from_secs(30);
        assert_eq!(
            run(&path, &["fake-tool".into()], None, t).unwrap(),
            ["one\ttab", "two"]
        );
        assert!(run(&path, &["missing-tool".into()], None, t).is_none());
        fs::write(root.join("fake-tool"), "#!/bin/sh\nexec /bin/cat\n").unwrap();
        assert_eq!(
            run(&path, &["fake-tool".into()], Some("from\nstdin"), t).unwrap(),
            ["from", "stdin"]
        );
        assert!(run("relative/dir", &["fake-tool".into()], None, t).is_none());
        fs::write(root.join("fake-tool"), "#!/bin/sh\nexit 3\n").unwrap();
        assert!(run(&path, &["fake-tool".into()], None, t).is_none());
        fs::remove_dir_all(root).unwrap();
    }
    fn alive(needle: &str) -> bool {
        Cmd::new("pgrep")
            .args(["-f", needle])
            .output()
            .unwrap()
            .status
            .success()
    }
    #[test]
    fn timeout_kills_descendants_that_hold_the_pipe() {
        let secs = format!("7{}.5", std::process::id() % 100000);
        // The shell forks `sleep` (the trailing `true` prevents an exec) and waits for it.
        let (root, path) = tool_dir("orphan", &format!("#!/bin/sh\n/bin/sleep {secs}\ntrue\n"));
        let start = Instant::now();
        assert!(
            run(
                &path,
                &["fake-tool".into()],
                None,
                Duration::from_millis(300)
            )
            .is_none()
        );
        assert!(start.elapsed() < Duration::from_secs(20));
        let sleep_cmd = format!("sleep {secs}");
        let gone = (0..200).any(|_| {
            thread::sleep(Duration::from_millis(25));
            !alive(&sleep_cmd)
        });
        assert!(gone, "grandchild `{sleep_cmd}` survived the timeout");
        fs::remove_dir_all(root).unwrap();
    }
}

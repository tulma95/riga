mod macos;
use riga::{
    background::Key as BgKey,
    completion::{self, Engine, Suggestion},
    protocol::{self, Message, State},
    session::Session,
    spec,
};
use std::{
    collections::{HashMap, HashSet},
    env, fs,
    io::{self, BufRead, BufReader, Read, Write},
    os::{
        fd::AsRawFd,
        unix::{
            fs::{MetadataExt, PermissionsExt},
            net::{UnixListener, UnixStream},
        },
    },
    path::PathBuf,
    sync::{Arc, Condvar, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};
unsafe extern "C" {
    fn getuid() -> u32;
    fn setsockopt(fd: i32, level: i32, name: i32, value: *const std::ffi::c_void, len: u32) -> i32;
    fn fcntl(fd: i32, cmd: i32, ...) -> i32;
    fn signal(sig: i32, handler: usize) -> usize;
}
fn socket_path() -> PathBuf {
    env::var_os("RIGA_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(format!("/tmp/riga-{}/socket", unsafe { getuid() })))
}
fn nonblock(fd: i32) -> io::Result<()> {
    unsafe {
        let size: i32 = 262144;
        let _ = setsockopt(
            fd,
            0xffff,
            0x1001,
            &size as *const _ as *const std::ffi::c_void,
            4,
        );
        let flags = fcntl(fd, 3);
        if flags < 0 || fcntl(fd, 4, flags | 4) < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}
enum Event {
    Connect(u64, UnixStream),
    Message(u64, Message),
    Disconnect(u64),
    Completed(String, u64, Vec<Suggestion>),
}
type Pending = Arc<(Mutex<HashMap<String, State>>, Condvar)>;
type Shared = Arc<Mutex<Waiters>>;
/// Which sessions' latest processed request waits on which background tool run.
/// Invariant: a session appears under a key only for its most recent processed state, and
/// `forget` drops it when its editing ends, so a finished run only wakes live sessions.
#[derive(Default)]
struct Waiters {
    latest: HashMap<String, State>,
    waiting: HashMap<BgKey, HashSet<String>>,
}
impl Waiters {
    fn record(&mut self, state: State, keys: Vec<BgKey>) {
        self.forget_waits(&state.session);
        for key in keys {
            self.waiting
                .entry(key)
                .or_default()
                .insert(state.session.clone());
        }
        self.latest.insert(state.session.clone(), state);
    }
    fn forget_waits(&mut self, session: &str) {
        self.waiting.retain(|_, set| {
            set.remove(session);
            !set.is_empty()
        });
    }
    fn forget(&mut self, session: &str) {
        self.forget_waits(session);
        self.latest.remove(session);
    }
    /// The states to recompute because `key` finished.
    fn wake(&mut self, key: &BgKey) -> Vec<State> {
        let sessions = self.waiting.remove(key).unwrap_or_default();
        sessions
            .iter()
            .filter_map(|s| self.latest.get(s).cloned())
            .collect()
    }
}
/// Queue `states` unless a newer request for the same session is already pending
/// (the main loop rejects results whose revision is no longer current).
fn requeue(pending: &Pending, states: Vec<State>) {
    if states.is_empty() {
        return;
    }
    let (lock, cv) = &**pending;
    let mut jobs = lock.lock().unwrap();
    for state in states {
        jobs.entry(state.session.clone()).or_insert(state);
    }
    cv.notify_one();
}
fn spawn_completion(tx: mpsc::SyncSender<Event>) -> (Pending, Shared) {
    let pending: Pending = Arc::new((Mutex::new(HashMap::new()), Condvar::new()));
    let waiters = Shared::default();
    let (p, w) = (pending.clone(), waiters.clone());
    thread::spawn(move || {
        let mut engine = Engine::default();
        // When a background tool run finishes, redo the requests that were waiting on it so
        // their values appear without another keystroke.
        let (wp, ww) = (p.clone(), w.clone());
        engine.on_background_update(move |key| {
            let states = ww.lock().unwrap().wake(key);
            requeue(&wp, states);
        });
        loop {
            let job = {
                let (lock, cv) = &*p;
                let mut jobs = lock.lock().unwrap();
                while jobs.is_empty() {
                    jobs = cv.wait(jobs).unwrap();
                }
                let key = jobs.keys().next().unwrap().clone();
                jobs.remove(&key).unwrap()
            };
            let items = engine.complete(&job);
            // Register before checking: a run that finished meanwhile is woken here, one that
            // finishes later is woken by its notification.
            let keys = engine.take_background_wanted();
            let done: Vec<_> = keys
                .iter()
                .filter(|k| !engine.background_running(k))
                .cloned()
                .collect();
            let states = {
                let mut w = w.lock().unwrap();
                w.record(job.clone(), keys);
                done.iter().flat_map(|k| w.wake(k)).collect()
            };
            requeue(&p, states);
            if tx
                .send(Event::Completed(job.session, job.revision, items))
                .is_err()
            {
                break;
            }
        }
    });
    (pending, waiters)
}
fn bind() -> io::Result<UnixListener> {
    let path = socket_path();
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("Socket must have a parent directory"))?;
    if !parent.exists() {
        fs::create_dir(parent)?;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
    let m = fs::symlink_metadata(parent)?;
    if !m.is_dir() || m.uid() != unsafe { getuid() } || m.permissions().mode() & 0o077 != 0 {
        return Err(io::Error::other(
            "Socket parent must be a real directory owned by you with mode 0700",
        ));
    }
    if path.exists() {
        if UnixStream::connect(&path).is_ok() {
            return Err(io::Error::other("A Riga server is already running"));
        }
        let m = fs::symlink_metadata(&path)?;
        use std::os::unix::fs::FileTypeExt;
        if !m.file_type().is_socket() || m.uid() != unsafe { getuid() } {
            return Err(io::Error::other(
                "Refusing to remove an unexpected socket path",
            ));
        }
        fs::remove_file(&path)?;
    }
    let listener = UnixListener::bind(path)?;
    fs::set_permissions(socket_path(), fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}
fn start_listener(listener: UnixListener, tx: mpsc::SyncSender<Event>) {
    thread::spawn(move || {
        for (next, stream) in listener.incoming().flatten().enumerate() {
            let id = next as u64 + 1;
            let Ok(writer) = stream.try_clone() else {
                continue;
            };
            if nonblock(writer.as_raw_fd()).is_err() {
                continue;
            }
            // Reader uses nonblocking socket too; bounded incremental frames prevent a client allocating unbounded memory.
            if tx.send(Event::Connect(id, writer)).is_err() {
                break;
            }
            let tx = tx.clone();
            thread::spawn(move || {
                let mut reader = BufReader::new(stream);
                let mut frame = Vec::new();
                loop {
                    match reader
                        .by_ref()
                        .take((65537 - frame.len()) as u64)
                        .read_until(b'\n', &mut frame)
                    {
                        Ok(0) => break,
                        Ok(_) => {
                            if frame.len() > 65536 {
                                break;
                            }
                            if frame.last() == Some(&b'\n') {
                                frame.pop();
                                if let Ok(line) = std::str::from_utf8(&frame)
                                    && let Some(message) = protocol::parse(line)
                                    && tx.send(Event::Message(id, message)).is_err()
                                {
                                    break;
                                }
                                frame.clear();
                            }
                        }
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(5));
                        }
                        Err(_) => break,
                    }
                    if frame.len() > 65536 {
                        break;
                    }
                }
                let _ = tx.send(Event::Disconnect(id));
            });
        }
    });
}
fn send(clients: &mut HashMap<u64, UnixStream>, id: u64, line: String) {
    if let Some(stream) = clients.get_mut(&id) {
        match stream.write(line.as_bytes()) {
            Ok(n) if n == line.len() => {}
            _ => {
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
        }
    }
}
fn serve() -> io::Result<()> {
    let listener = bind()?;
    unsafe {
        signal(13, 1);
    }
    let (tx, rx) = mpsc::sync_channel(256);
    let (pending, waiters) = spawn_completion(tx.clone());
    start_listener(listener, tx);
    let _pool = macos::Pool::new();
    let mut desktop = macos::Desktop::new();
    desktop.show_status_item();
    let mut clients = HashMap::new();
    let mut sessions: HashMap<String, Session> = HashMap::new();
    let mut hints: HashMap<u64, String> = HashMap::new();
    let mut last_probe = Instant::now() - Duration::from_secs(1);
    let mut geometry = None;
    eprintln!(
        "Riga listening at {}. No command buffers are logged.",
        socket_path().display()
    );
    if !macos::Desktop::trusted() {
        eprintln!(
            "Accessibility permission missing. Run `riga doctor` for setup. Popup stays hidden."
        );
    }
    loop {
        let _iteration_pool = macos::Pool::new();
        for _ in 0..256 {
            let Ok(event) = rx.try_recv() else {
                break;
            };
            match event {
                Event::Connect(id, stream) => {
                    clients.insert(id, stream);
                }
                Event::Disconnect(id) => {
                    clients.remove(&id);
                    hints.remove(&id);
                    let mut jobs = pending.0.lock().unwrap();
                    for (key, s) in &sessions {
                        if s.connection == id {
                            jobs.remove(key);
                            waiters.lock().unwrap().forget(key);
                        }
                    }
                    sessions.retain(|_, s| s.connection != id);
                }
                Event::Message(id, Message::State(state)) => {
                    if sessions.len() >= 128 && !sessions.contains_key(&state.session) {
                        continue;
                    }
                    let accepted = if let Some(s) = sessions.get_mut(&state.session) {
                        s.update(id, state.clone())
                    } else if sessions.values().any(|s| s.connection == id) {
                        false
                    } else {
                        sessions.insert(state.session.clone(), Session::new(id, state.clone()));
                        true
                    };
                    if accepted {
                        send(&mut clients, id, format!("P\t{}\t0\n", state.revision));
                        let (lock, cv) = &*pending;
                        lock.lock().unwrap().insert(state.session.clone(), state);
                        cv.notify_one();
                    }
                }
                Event::Message(id, Message::Keys(keys)) => {
                    hints.insert(id, macos::key_hints(&keys));
                }
                Event::Message(id, Message::End(key, revision)) => {
                    if let Some(s) = sessions.get_mut(&key)
                        && s.connection == id
                        && revision >= s.state.revision
                    {
                        s.end(revision);
                        pending.0.lock().unwrap().remove(&key);
                        waiters.lock().unwrap().forget(&key);
                        send(&mut clients, id, format!("P\t{}\t0\n", revision));
                    }
                }
                Event::Completed(key, revision, items) => {
                    if let Some(s) = sessions.get_mut(&key) {
                        s.apply(revision, items);
                    }
                }
                Event::Message(id, Message::Action(key, revision, action)) => {
                    // Revalidate focus/pane/caret synchronously for actions, never trust a cached visible bit.
                    geometry = desktop.probe();
                    last_probe = Instant::now();
                    if let Some(s) = sessions.get_mut(&key) {
                        let eligible = s.connection == id
                            && s.state.revision == revision
                            && !s.stale
                            && geometry.as_ref().is_some_and(|g| s.eligible(&g.pane));
                        if !eligible {
                            send(&mut clients, id, format!("P\t{revision}\t0\n"));
                            continue;
                        }
                        match action.as_str() {
                            "next" => s.selected = (s.selected + 1) % s.items.len(),
                            "prev" => s.selected = (s.selected + s.items.len() - 1) % s.items.len(),
                            "dismiss" => {
                                s.dismissed = true;
                                s.visible = false;
                                send(&mut clients, id, format!("P\t{revision}\t0\n"));
                            }
                            "accept" if s.items[s.selected].kind == completion::STATUS => {
                                s.dismissed = true;
                                s.visible = false;
                                send(&mut clients, id, format!("P\t{revision}\t0\n"));
                            }
                            "accept" => {
                                let item = &s.items[s.selected];
                                send(
                                    &mut clients,
                                    id,
                                    format!(
                                        "I\t{revision}\t{}\t{}\t{}\n",
                                        item.start,
                                        item.end,
                                        protocol::encode(&item.insert)
                                    ),
                                );
                                s.dismissed = true;
                                s.visible = false;
                                send(&mut clients, id, format!("P\t{revision}\t0\n"));
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        let has_candidates = sessions
            .values()
            .any(|s| s.editing && !s.dismissed && !s.items.is_empty());
        if !has_candidates {
            geometry = None;
        } else if last_probe.elapsed() >= Duration::from_millis(70) {
            geometry = desktop.probe();
            last_probe = Instant::now();
        }
        // A pane may contain nested shells. Only its most recently connected live editor is eligible.
        let active = geometry.as_ref().and_then(|g| {
            sessions
                .iter()
                .filter(|(_, s)| s.eligible(&g.pane))
                .max_by_key(|(_, s)| s.connection)
                .map(|(k, _)| k.clone())
        });
        if let Some(ref key) = active {
            if let (Some(s), Some(g)) = (sessions.get(key), geometry.as_ref()) {
                let hint = hints.get(&s.connection).map_or("", String::as_str);
                desktop.show(g, &s.items, s.selected, hint);
            }
        } else {
            desktop.hide();
        }
        for (key, s) in &mut sessions {
            let visible = active.as_ref() == Some(key) && !s.stale;
            if s.visible != visible {
                s.visible = visible;
                send(
                    &mut clients,
                    s.connection,
                    format!(
                        "P\t{}\t{}\n",
                        s.state.revision,
                        if visible { s.items.len() } else { 0 }
                    ),
                );
            }
        }
        desktop.pump();
        thread::sleep(Duration::from_millis(12));
    }
}
fn doctor() {
    let _pool = macos::Pool::new();
    println!(
        "Riga {} | macOS {} | architecture {}",
        env!("CARGO_PKG_VERSION"),
        std::process::Command::new("sw_vers")
            .arg("-productVersion")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
            .unwrap_or_default(),
        env::consts::ARCH
    );
    println!("Accessibility trusted: {}", macos::Desktop::trusted());
    println!(
        "Socket: {} ({})",
        socket_path().display(),
        if UnixStream::connect(socket_path()).is_ok() {
            "running"
        } else {
            "not connected"
        }
    );
    println!("Specs: {}", Engine::default().spec_status().join(", "));
    println!(
        "Setup: System Settings → Privacy & Security → Accessibility: allow the app/terminal launching Riga; restart Riga after changing permission. Automation: allow Riga's launcher to control iTerm2 when prompted. No global keyboard monitor is used.\nRun `riga probe` from iTerm2 to check actual caret geometry and pane identity."
    );
}
/// Validate spec files and run their embedded tests (for developing bundled specs).
fn check_spec(files: &[String]) -> io::Result<()> {
    if files.is_empty() {
        return Err(io::Error::other("Usage: riga check-spec FILE..."));
    }
    let paths: Vec<PathBuf> = files.iter().map(PathBuf::from).collect();
    let mut failed = false;
    for path in paths {
        let label = spec::printable(&path.display().to_string());
        let errors = match spec::read(&path) {
            Ok(spec) => completion::spec_test_failures(&spec, &label),
            Err(errors) => errors,
        };
        if errors.is_empty() {
            println!("{label}: ok");
        }
        for error in &errors {
            println!("{error}");
        }
        failed |= !errors.is_empty();
    }
    if failed {
        std::process::exit(1);
    }
    Ok(())
}
/// Dry run with the cursor at the end; the buffer comes from the command line, never the socket.
fn complete(args: &[String]) -> io::Result<()> {
    let (cwd, buffer) = match args {
        [flag, dir, buffer] if flag == "--cwd" => (PathBuf::from(dir), buffer),
        [buffer] => (env::current_dir()?, buffer),
        _ => return Err(io::Error::other("Usage: riga complete [--cwd DIR] BUFFER")),
    };
    let state = State {
        session: "cli".into(),
        pane: "cli".into(),
        revision: 1,
        cursor: buffer.chars().count(),
        cwd,
        buffer: buffer.clone(),
        path: env::var("PATH").unwrap_or_default(),
    };
    let mut engine = Engine::default();
    let (tx, rx) = mpsc::channel();
    engine.on_background_update(move |_| {
        let _ = tx.send(());
    });
    let mut items = engine.complete(&state);
    // A one-shot run has no next keystroke: wait (up to 10 s, for a model run) for background tool runs to finish,
    // then retry. Each finished run sends a notification after clearing its busy flag.
    if engine.background_busy() {
        let deadline = Instant::now() + Duration::from_secs(10);
        while engine.background_busy() {
            let left = deadline.saturating_duration_since(Instant::now());
            if rx.recv_timeout(left).is_err() {
                break;
            }
        }
        items = engine.complete(&state);
    }
    let mut out = io::stdout().lock();
    for s in items {
        let fields = [s.label.as_str(), s.kind, &s.description, &s.insert];
        match writeln!(out, "{}", fields.map(protocol::encode).join("\t")) {
            Err(e) if e.kind() == io::ErrorKind::BrokenPipe => return Ok(()),
            r => r?,
        }
    }
    match out.flush() {
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        r => r,
    }
}
fn main() {
    let args: Vec<_> = env::args().collect();
    let result = match args.get(1).map(String::as_str) {
        Some("serve") => serve(),
        Some("nonblock") => args
            .get(2)
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| io::Error::other("Expected fd"))
            .and_then(nonblock),
        Some("doctor") => {
            doctor();
            Ok(())
        }
        Some("check-spec") => check_spec(&args[2..]),
        Some("complete") => complete(&args[2..]),
        Some("probe") => {
            let _pool = macos::Pool::new();
            let mut d = macos::Desktop::new();
            match d.probe() {
                Some(g) => println!(
                    "pane={} caret x={} y={} width={} height={}",
                    g.pane,
                    g.caret.origin.x,
                    g.caret.origin.y,
                    g.caret.size.width,
                    g.caret.size.height
                ),
                None => println!("Popup suppressed: {}", d.reason),
            }
            Ok(())
        }
        _ => {
            println!(
                "Riga: local iTerm2/zsh autocomplete\nUsage: riga serve | doctor | probe | check-spec FILE... | complete [--cwd DIR] BUFFER\nBuild: cargo build --release\nShell: source shell/riga.zsh"
            );
            Ok(())
        }
    };
    if let Err(e) = result {
        eprintln!("riga: {e}");
        std::process::exit(1);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn state(session: &str, revision: u64) -> State {
        State {
            session: session.into(),
            pane: "p".into(),
            revision,
            cursor: 0,
            cwd: PathBuf::new(),
            buffer: String::new(),
            path: String::new(),
        }
    }
    fn key(n: &str) -> BgKey {
        ("/p".into(), vec![n.into()])
    }
    #[test]
    fn finished_run_wakes_only_sessions_that_waited_on_it() {
        let mut w = Waiters::default();
        w.record(state("a", 1), vec![key("tmux")]);
        w.record(state("b", 5), vec![]); // B typed later and needs nothing
        let woken = w.wake(&key("tmux"));
        assert_eq!(woken.len(), 1);
        assert_eq!((woken[0].session.as_str(), woken[0].revision), ("a", 1));
        assert!(w.wake(&key("tmux")).is_empty());
    }
    #[test]
    fn newer_state_and_ended_sessions_drop_their_waits() {
        let mut w = Waiters::default();
        w.record(state("a", 1), vec![key("x")]);
        w.record(state("a", 2), vec![key("y")]);
        assert!(w.wake(&key("x")).is_empty());
        assert_eq!(w.wake(&key("y"))[0].revision, 2);
        w.record(state("a", 3), vec![key("z")]);
        w.forget("a");
        assert!(w.wake(&key("z")).is_empty() && w.latest.is_empty());
    }
    #[test]
    fn requeue_never_overrides_a_newer_pending_request() {
        let pending: Pending = Arc::default();
        pending.0.lock().unwrap().insert("a".into(), state("a", 9));
        requeue(&pending, vec![state("a", 1), state("b", 2)]);
        let jobs = pending.0.lock().unwrap();
        assert_eq!((jobs["a"].revision, jobs["b"].revision), (9, 2));
    }
}

//! tmux session, window, pane and buffer names from the default server, listed in the background.
use crate::{
    names::{Names, Ref},
    sources::Env,
};
use std::time::Duration;
const MAX_NAMES: usize = 5_000;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Session,
    Window,
    Pane,
    Buffer,
}
// ponytail: always the default server; -L/-S in the buffer and the shell's TMUX_TMPDIR are ignored.
pub fn complete(env: &Env, kind: Kind) -> Vec<Ref> {
    let (command, format) = match kind {
        Kind::Session => (
            "list-sessions",
            "#{session_name}\twindows: #{session_windows}",
        ),
        Kind::Window => (
            "list-windows",
            "#{session_name}:#{window_index}\t#{window_name}",
        ),
        Kind::Pane => (
            "list-panes",
            "#{session_name}:#{window_index}.#{pane_index}\t#{pane_current_command}",
        ),
        Kind::Buffer => ("list-buffers", "#{buffer_name}\t#{buffer_size} bytes"),
    };
    let mut argv = vec!["tmux", command, "-F", format];
    if matches!(kind, Kind::Window | Kind::Pane) {
        argv.push("-a");
    }
    parse(&env.runs.lines(env.path, &argv, Duration::from_secs(5)))
}
/// `name<TAB>description` lines.
fn parse(lines: &[String]) -> Vec<Ref> {
    let mut out = Names::new(MAX_NAMES);
    for l in lines {
        let (name, description) = l.split_once('\t').unwrap_or((l, ""));
        out.push(name, description);
    }
    out.finish()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_name_and_description() {
        let lines = [
            "dev:1\tvim".to_owned(),
            "my session\t2 windows".into(),
            "\tx".into(),
            "solo".into(),
        ];
        let refs = parse(&lines);
        // Sorted by name.
        let pairs: Vec<_> = refs
            .iter()
            .map(|r| (r.name.as_str(), r.description.as_str()))
            .collect();
        assert_eq!(
            pairs,
            [("dev:1", "vim"), ("my session", "2 windows"), ("solo", "")]
        );
    }
    #[test]
    fn engine_lists_sessions_from_shell_path_without_waiting() {
        use crate::{completion::Engine, protocol::State};
        use std::{fs, os::unix::fs::PermissionsExt, time::Instant};
        let root = std::env::temp_dir().join(format!("riga-tmux-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let tool = root.join("tmux");
        // The tool blocks reading a fifo until the test has seen `complete` return empty.
        let fifo = root.join("gate");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );
        let script = format!(
            "#!/bin/sh\nread _ < '{}'\n[ \"$1\" = list-sessions ] && printf 'dev\\twindows: 2\\n'\n",
            fifo.display()
        );
        fs::write(&tool, script).unwrap();
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
        let mut engine = Engine::new(String::new(), root.clone());
        let buffer = "tmux attach -t ";
        let state = State {
            session: "s".into(),
            pane: "p".into(),
            revision: 1,
            cursor: buffer.len(),
            cwd: root.clone(),
            buffer: buffer.into(),
            path: root.to_string_lossy().into_owned(),
        };
        assert!(engine.complete(&state).is_empty());
        assert!(engine.background_busy());
        fs::write(&fifo, "go\n").unwrap(); // opens (blocking) until the tool reads
        let start = Instant::now();
        while engine.background_busy() {
            assert!(
                start.elapsed() < Duration::from_secs(30),
                "tmux run never finished"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let items = engine.complete(&state);
        assert_eq!(items.len(), 1);
        assert_eq!(
            (items[0].label.as_str(), items[0].description.as_str()),
            ("dev", "windows: 2")
        );
        fs::remove_dir_all(root).unwrap();
    }
}

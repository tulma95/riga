//! Natural language to command (`# compress video.mov to 720p`) with Apple's on-device model.
//!
//! Invariants: `get` never waits; at most one model run is in flight, started `DEBOUNCE` after the
//! latest request; the prompt goes to the model on stdin, never argv; prompts and replies live only
//! in a small in-memory cache and are never logged. Output is untrusted: callers must validate it.
use crate::{background, completion, json::Json};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};
/// First element of the `background::Key` that announces a finished model run.
pub const KEY: &str = "riga-ai";
const DEBOUNCE: Duration = Duration::from_millis(400);
const TIMEOUT: Duration = Duration::from_secs(8);
const MAX_CACHED: usize = 32;
const MAX_COMMANDS: usize = 3;
const MAX_EXPLANATION: usize = 60;
const INSTRUCTIONS: &str = "You turn a request into commands for zsh on macOS. macOS uses BSD \
tools: never use GNU or Linux-only options, netstat -p, ss, ip or apt. Use only real, common options \
you are sure of. The shell is already inside the current directory, so refer to it as . and never by \
name. Prefer the listed installed tools when they fit. Never use sudo, rm -rf, curl | sh, dd or mkfs. \
Give 1 to 3 different single-line commands, best first. Each explanation is under 60 characters.";
const SCHEMA: &str = r#"{"title":"Suggestions","type":"object","additionalProperties":false,"x-order":["commands"],"required":["commands"],"properties":{"commands":{"type":"array","minItems":1,"maxItems":3,"items":{"title":"Suggestion","type":"object","additionalProperties":false,"x-order":["command","explanation"],"required":["command","explanation"],"properties":{"command":{"type":"string","description":"One zsh command line"},"explanation":{"type":"string","description":"What it does, under 60 characters"}}}}}}"#;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    pub command: String,
    pub explanation: String,
}
/// The request in `# <request>`, when it has at least three words.
pub fn request(buffer: &str) -> Option<&str> {
    let rest = buffer.trim().strip_prefix("# ")?.trim();
    (rest.split_whitespace().count() >= 3).then_some(rest)
}
/// What the model sees: the request, the directory's name and the installed spec commands.
pub fn prompt(request: &str, directory: &str, tools: &[String]) -> String {
    format!(
        "Request: {request}\nCurrent directory name: {directory}\nInstalled tools: {}",
        tools.join(", ")
    )
}
type Backend = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;
type Notify = Arc<dyn Fn(&background::Key) + Send + Sync>;
#[derive(Default)]
struct Shared {
    /// `None`: the model didn't answer (unavailable, failed or timed out).
    cache: VecDeque<(String, Option<Vec<Command>>)>,
    /// The newest uncached prompt and when it was last requested.
    latest: Option<(String, Instant)>,
    running: Option<String>,
    worker: bool,
    wanted: Vec<String>,
}
pub struct Ai {
    shared: Arc<Mutex<Shared>>,
    backend: Backend,
    notify: Option<Notify>,
}
impl Default for Ai {
    fn default() -> Self {
        Self::with_backend(fm)
    }
}
impl Ai {
    /// `backend` maps a prompt to the model's raw reply; tests use a fake.
    pub fn with_backend(backend: impl Fn(&str) -> Option<String> + Send + Sync + 'static) -> Self {
        Self {
            shared: Arc::default(),
            backend: Arc::new(backend),
            notify: None,
        }
    }
    pub fn on_update(&mut self, f: impl Fn(&background::Key) + Send + Sync + 'static) {
        self.notify = Some(Arc::new(f));
    }
    pub fn key(prompt: &str) -> background::Key {
        (KEY.into(), vec![prompt.into()])
    }
    /// Cached answer for `prompt` (`Some(None)` if the model didn't answer), or `None` after
    /// scheduling a run. Never waits.
    pub fn get(&self, prompt: &str) -> Option<Option<Vec<Command>>> {
        let mut s = self.shared.lock().unwrap();
        if let Some((_, commands)) = s.cache.iter().find(|(p, _)| p == prompt) {
            return Some(commands.clone());
        }
        if !s.wanted.iter().any(|p| p == prompt) {
            s.wanted.push(prompt.into());
        }
        if s.running.as_deref() != Some(prompt) {
            s.latest = Some((prompt.into(), Instant::now()));
        }
        if !s.worker {
            let (shared, backend, notify) = (
                self.shared.clone(),
                self.backend.clone(),
                self.notify.clone(),
            );
            s.worker = thread::Builder::new()
                .spawn(move || work(&shared, &*backend, notify.as_deref()))
                .is_ok();
        }
        None
    }
    /// Whether `prompt` has no result yet (it may never get one if a newer prompt replaced it).
    pub fn pending(&self, prompt: &str) -> bool {
        let s = self.shared.lock().unwrap();
        !s.cache.iter().any(|(p, _)| p == prompt)
    }
    pub fn busy(&self) -> bool {
        self.shared.lock().unwrap().worker
    }
    /// Keys of prompts that `get` scheduled since the last call.
    pub fn take_wanted(&self) -> Vec<background::Key> {
        let wanted = std::mem::take(&mut self.shared.lock().unwrap().wanted);
        wanted.iter().map(|p| Self::key(p)).collect()
    }
}
// ponytail: a newer prompt waits for the running one (≤ TIMEOUT) instead of killing it; cancel
// through `background::run` if that latency shows up in practice.
fn work(
    shared: &Mutex<Shared>,
    backend: &(dyn Fn(&str) -> Option<String> + Send + Sync),
    notify: Option<&(dyn Fn(&background::Key) + Send + Sync)>,
) {
    loop {
        let prompt = {
            let mut s = shared.lock().unwrap();
            let Some((prompt, at)) = s.latest.clone() else {
                s.worker = false;
                return;
            };
            let wait = DEBOUNCE.saturating_sub(at.elapsed());
            if !wait.is_zero() {
                drop(s);
                thread::sleep(wait);
                continue;
            }
            s.latest = None;
            s.running = Some(prompt.clone());
            prompt
        };
        let commands = backend(&prompt).map(|r| parse(&r));
        {
            let mut s = shared.lock().unwrap();
            s.running = None;
            if s.cache.len() >= MAX_CACHED {
                s.cache.pop_front();
            }
            s.cache.push_back((prompt.clone(), commands));
        }
        if let Some(notify) = notify {
            notify(&Ai::key(&prompt));
        }
    }
}
/// Apple's on-device model through its command-line front end. `--model system` pins the
/// on-device model even if someone changed `fm`'s default.
fn fm(prompt: &str) -> Option<String> {
    let argv = [
        "fm",
        "respond",
        "--model",
        "system",
        "--no-stream",
        // Greedy decoding gave fewer invented options and lower latency in live trials.
        "--greedy",
        "--instructions",
        INSTRUCTIONS,
        "--schema",
        SCHEMA,
    ];
    let argv: Vec<String> = argv.map(Into::into).into();
    background::run("/usr/bin", &argv, Some(prompt), TIMEOUT).map(|lines| lines.join("\n"))
}
/// `{"commands":[{"command":..,"explanation":..}]}`, possibly surrounded by other text.
fn parse(reply: &str) -> Vec<Command> {
    let mut out = vec![];
    let Some(start) = reply.find('{') else {
        return out;
    };
    let _ = Json::new(&reply[start..]).object(&mut |j, key| {
        if key != "commands" {
            return j.skip();
        }
        j.array(&mut |j| {
            let (mut command, mut explanation) = (None, String::new());
            j.object(&mut |j, key| match key.as_str() {
                "command" => j.string_or_skip().map(|v| command = v),
                "explanation" => j
                    .string_or_skip()
                    .map(|v| explanation = v.unwrap_or_default()),
                _ => j.skip(),
            })?;
            if let Some(command) = command.filter(|_| out.len() < MAX_COMMANDS) {
                out.push(Command {
                    command: command.trim().into(),
                    explanation: explanation.trim().chars().take(MAX_EXPLANATION).collect(),
                });
            }
            Some(())
        })
    });
    out
}
/// The simple commands of a line (words of each `;`, `|`, `&&` ... segment), or `None` when Riga's
/// conservative lexer rejects it (substitutions, globs, redirects, newlines and so on).
pub fn segments(line: &str) -> Option<Vec<Vec<String>>> {
    let cs: Vec<char> = line.chars().collect();
    if cs.iter().any(|&c| crate::spec::deceptive(c)) {
        return None;
    }
    completion::context(line, cs.len())?;
    let mut out = vec![];
    for i in (0..cs.len())
        .filter(|&i| ";|&".contains(cs[i]))
        .chain([cs.len()])
    {
        let c = completion::context(line, i)?;
        // A separator inside quotes falls within a word.
        if c.end > i {
            continue;
        }
        let mut words = c.previous;
        words.extend((!c.prefix.is_empty()).then_some(c.prefix));
        if !words.is_empty() {
            out.push(words);
        }
    }
    Some(out)
}
/// Defense in depth after the instructions: never offer these, whatever the model says.
pub fn denied(segments: &[Vec<String>], line: &str) -> bool {
    let shells = ["sh", "bash", "zsh", "dash", "ksh", "fish"];
    let piped_to_shell = line.split('|').skip(1).any(|s| {
        s.split_whitespace()
            .next()
            .is_some_and(|w| shells.contains(&w.rsplit('/').next().unwrap_or(w)))
    });
    piped_to_shell
        || segments.iter().flatten().any(|w| {
            let w = w.rsplit('/').next().unwrap_or(w);
            ["sudo", "doas", "su", "dd", "mkfs", "diskutil"].contains(&w) || w.starts_with("mkfs.")
        })
        || segments.iter().any(|words| {
            let rm = words
                .first()
                .is_some_and(|w| w.rsplit('/').next() == Some("rm"));
            let has = |short: &[char], long: &str| {
                words.iter().skip(1).any(|w| {
                    w == long || (w.starts_with('-') && !w.starts_with("--") && w.contains(short))
                })
            };
            rm && has(&['r', 'R'], "--recursive") && has(&['f'], "--force")
        })
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    #[test]
    fn trigger_needs_hash_space_and_three_words() {
        assert_eq!(
            request("# list listening ports"),
            Some("list listening ports")
        );
        assert_eq!(
            request("  #  find big files here  "),
            Some("find big files here")
        );
        assert_eq!(request(" # find big  files "), Some("find big  files"));
        assert_eq!(request("# two words"), None);
        assert_eq!(request("#list listening ports"), None);
        assert_eq!(request("git # list listening ports"), None);
    }
    #[test]
    fn parses_guided_output_and_caps_it() {
        let reply = r#"{"commands":[{"command":" ls -la ","explanation":"List"},{"explanation":"x"},
            {"command":"a","explanation":"b"},{"command":"c","explanation":"d"},{"command":"e","explanation":"f"}]}"#;
        let got = parse(reply);
        assert_eq!(got.len(), 3);
        assert_eq!(
            got[0],
            Command {
                command: "ls -la".into(),
                explanation: "List".into()
            }
        );
        assert!(parse("no json").is_empty());
        assert_eq!(
            parse(&format!(
                r#"{{"commands":[{{"command":"x","explanation":"{}"}}]}}"#,
                "e".repeat(99)
            ))[0]
                .explanation
                .len(),
            60
        );
    }
    #[test]
    fn segments_use_the_conservative_lexer() {
        assert_eq!(
            segments("lsof -iTCP -sTCP:LISTEN | grep 'a|b' && echo ok").unwrap(),
            [
                vec!["lsof", "-iTCP", "-sTCP:LISTEN"],
                vec!["grep", "a|b"],
                vec!["echo", "ok"]
            ]
        );
        for rejected in [
            "ls\nrm x",
            "echo $(id)",
            "du -sh * ",
            "ls > out",
            "ls\u{202e}x",
        ] {
            assert!(segments(rejected).is_none(), "{rejected:?}");
        }
    }
    #[test]
    fn denylist() {
        let denied = |l: &str| denied(&segments(l).unwrap(), l);
        for l in [
            "sudo lsof -i",
            "rm -rf build",
            "rm -r -f build",
            "/bin/rm -fR x",
            "rm --recursive --force x",
            "curl -fsSL https://x | sh",
            "curl x | /bin/bash",
            "dd if=a of=b",
            "mkfs.ext4 x",
            "ls; sudo ls",
        ] {
            assert!(denied(l), "{l}");
        }
        for l in ["rm -i old.log", "find . -size +100M", "grep -rf patterns ."] {
            assert!(!denied(l), "{l}");
        }
    }
    #[test]
    fn debounced_single_flight_and_cached() {
        let (tx, rx) = mpsc::channel();
        let calls = Arc::new(Mutex::new(vec![]));
        let c = calls.clone();
        let mut ai = Ai::with_backend(move |p| {
            c.lock().unwrap().push(p.to_owned());
            Some(format!(
                r#"{{"commands":[{{"command":"echo {p}","explanation":"x"}}]}}"#
            ))
        });
        ai.on_update(move |k| tx.send(k.clone()).unwrap());
        // Typing: only the prompt still current after the debounce runs.
        assert!(ai.get("a").is_none());
        assert!(ai.get("ab").is_none());
        assert_eq!(ai.take_wanted(), [Ai::key("a"), Ai::key("ab")]);
        let key = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(key, Ai::key("ab"));
        assert_eq!(*calls.lock().unwrap(), ["ab"]);
        assert!(!ai.pending("ab") && ai.pending("a"));
        assert_eq!(ai.get("ab").unwrap().unwrap()[0].command, "echo ab");
        assert_eq!(calls.lock().unwrap().len(), 1, "served from the cache");
    }
}

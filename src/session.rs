use crate::{completion::Suggestion, protocol::State};
#[derive(Clone)]
pub struct Session {
    pub connection: u64,
    pub state: State,
    pub editing: bool,
    pub dismissed: bool,
    pub items: Vec<Suggestion>,
    pub selected: usize,
    pub visible: bool,
    /// Items belong to an older revision: still drawn so the popup does not flash between
    /// keystrokes, but never accepted or announced to the shell.
    pub stale: bool,
}
impl Session {
    pub fn new(connection: u64, state: State) -> Self {
        Self {
            connection,
            state,
            editing: true,
            dismissed: false,
            items: vec![],
            selected: 0,
            visible: false,
            stale: false,
        }
    }
    pub fn update(&mut self, connection: u64, state: State) -> bool {
        if connection != self.connection
            || state.revision <= self.state.revision
            || state.pane != self.state.pane
        {
            return false;
        }
        self.state = state;
        if self.dismissed {
            self.items.clear();
        }
        self.editing = true;
        self.dismissed = false;
        self.stale = true;
        self.selected = 0;
        self.visible = false;
        true
    }
    pub fn apply(&mut self, revision: u64, items: Vec<Suggestion>) -> bool {
        if !self.editing || self.dismissed || revision != self.state.revision {
            return false;
        }
        // A refresh of already-current items (a background value arrived) keeps the user's
        // selection on the same item; fresh results for a new revision start at the top.
        let keep = (!self.stale)
            .then(|| self.items.get(self.selected).cloned())
            .flatten();
        self.selected = keep
            .and_then(|k| items.iter().position(|i| *i == k))
            .unwrap_or(0);
        self.items = items;
        self.stale = false;
        true
    }
    pub fn end(&mut self, revision: u64) {
        if revision >= self.state.revision {
            self.state.revision = revision;
            self.editing = false;
            self.items.clear();
            self.visible = false;
        }
    }
    pub fn eligible(&self, pane: &str) -> bool {
        self.editing
            && !self.dismissed
            && !self.items.is_empty()
            && self.state.pane.rsplit(':').next() == Some(pane)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn state(revision: u64) -> State {
        State {
            session: "s".into(),
            pane: "w0t0p0:ABC".into(),
            revision,
            cursor: 1,
            cwd: "/tmp".into(),
            buffer: "g".into(),
            path: String::new(),
        }
    }
    #[test]
    fn old_completions_never_win() {
        let mut s = Session::new(1, state(1));
        assert!(s.update(1, state(2)));
        assert!(!s.apply(1, vec![]));
        assert!(!s.update(1, state(1)));
        assert!(!s.update(2, state(3)));
        assert!(s.apply(2, vec![]));
        s.end(3);
        assert!(!s.apply(2, vec![]));
    }
    #[test]
    fn dismissed_and_other_panes_hidden() {
        let mut s = Session::new(1, state(1));
        s.items.push(Suggestion {
            label: "git".into(),
            kind: "command",
            description: String::new(),
            insert: "git".into(),
            start: 0,
            end: 1,
        });
        assert!(s.eligible("ABC"));
        assert!(!s.eligible("DEF"));
        s.dismissed = true;
        assert!(!s.eligible("ABC"));
        assert!(!s.apply(1, vec![]));
    }
    #[test]
    fn edits_keep_stale_items_until_fresh_results() {
        let mut s = Session::new(1, state(1));
        assert!(s.apply(
            1,
            vec![Suggestion {
                label: "git".into(),
                kind: "command",
                description: String::new(),
                insert: "git".into(),
                start: 0,
                end: 1,
            }]
        ));
        assert!(s.update(1, state(2)));
        assert!(s.stale && s.eligible("ABC"));
        assert!(s.apply(2, vec![]));
        assert!(!s.stale && !s.eligible("ABC"));
        s.dismissed = true;
        s.items = vec![Suggestion {
            label: "x".into(),
            kind: "command",
            description: String::new(),
            insert: "x".into(),
            start: 0,
            end: 1,
        }];
        assert!(s.update(1, state(3)));
        assert!(!s.eligible("ABC"));
    }
    fn item(label: &str) -> Suggestion {
        Suggestion {
            label: label.into(),
            kind: "value",
            description: String::new(),
            insert: label.into(),
            start: 0,
            end: 1,
        }
    }
    #[test]
    fn same_revision_refresh_keeps_selected_item() {
        let mut s = Session::new(1, state(1));
        assert!(s.apply(1, vec![item("a"), item("b"), item("c")]));
        s.selected = 1;
        assert!(s.apply(1, vec![item("x"), item("a"), item("b")]));
        assert_eq!(s.items[s.selected].label, "b");
        // Selected item vanished: back to the top rather than pointing past the end.
        s.selected = 2;
        assert!(s.apply(1, vec![item("x")]));
        assert_eq!(s.selected, 0);
        // A newer revision always starts at the top; outdated results are still rejected.
        s.selected = 0;
        assert!(s.update(1, state(2)));
        assert!(!s.apply(1, vec![item("x"), item("y")]));
        assert!(s.apply(2, vec![item("y"), item("x")]));
        assert_eq!(s.selected, 0);
    }
}

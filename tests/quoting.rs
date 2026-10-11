//! Exercise emitted words in the actual target shell, rather than duplicating its parser.
use riga::completion::quote_word;
use std::process::Command;

#[test]
fn quoted_words_roundtrip_through_zsh_without_expansion() {
    for word in [
        "",
        "hello world/",
        "it's a directory/",
        "a\"b\\c",
        "$(print EXPANDED)",
        "`print EXPANDED`",
        "$HOME",
        "~/literal",
        "*?[abc]{one,two}",
        "semi;colon & pipe|redirect>file",
        "first\nsecond\tthird",
        "café 世界 🦀 e\u{301}",
        "-n",
        "#comment",
        "!history",
        "=ls",
        "=riga_nonexistent_test_executable",
        "ENV=value",
    ] {
        let script = format!("print -rn -- {}", quote_word(word));
        let result = Command::new("/bin/zsh")
            .args(["-f", "-c", &script])
            .output()
            .expect("target zsh must be installed");
        assert!(result.status.success(), "zsh rejected {word:?}");
        assert_eq!(result.stdout, word.as_bytes(), "changed {word:?}");
        assert!(result.stderr.is_empty(), "stderr for {word:?}");
    }
}

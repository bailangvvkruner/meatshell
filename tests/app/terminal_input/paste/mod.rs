use super::super::*;

#[test]
fn paste_normalizes_newlines_to_cr() {
    assert_eq!(
        normalize_pasted_newlines("sudo apt install \\\r\n  docker-ce"),
        "sudo apt install \\\r  docker-ce"
    );
    assert_eq!(normalize_pasted_newlines("a\nb\nc"), "a\rb\rc");
    assert_eq!(normalize_pasted_newlines("a\rb"), "a\rb");
    assert_eq!(normalize_pasted_newlines("echo hi"), "echo hi");
}

#[test]
fn command_bar_preserves_multiline_heredoc() {
    let command = "cat <<'EOF'\nHEREDOC-1\n中文-HEREDOC-2\nEOF\n";
    let (history, bytes) = encode_command_bar_input(command).unwrap();
    assert_eq!(history, command.trim_end());
    assert_eq!(bytes, command.as_bytes());
    assert!(!history.lines().any(|line| line.starts_with(' ')));
}

#[test]
fn paste_uses_remote_bracketed_paste_mode() {
    assert_eq!(
        encode_pasted_text("first\r\n  second", true),
        b"\x1b[200~first\r  second\x1b[201~"
    );
    assert_eq!(
        encode_pasted_text("safe\x1b[201~\x03text", true),
        b"\x1b[200~safe[201~text\x1b[201~"
    );
    assert_eq!(
        encode_pasted_text("first\r\nsecond", false),
        b"first\rsecond"
    );
}

#[test]
fn long_pastes_switch_to_large_review() {
    assert!(!paste_requires_large_review("short prompt\nsecond line"));
    assert!(!paste_requires_large_review(&"a".repeat(600)));
    assert!(paste_requires_large_review(&"a".repeat(601)));
    assert!(!paste_requires_large_review(&vec!["line"; 12].join("\r\n")));
    assert!(paste_requires_large_review(&vec!["line"; 13].join("\r\n")));
}

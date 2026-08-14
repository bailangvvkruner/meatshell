use super::*;

#[test]
fn paste_tracks_remote_bracketed_paste_state() {
    let bufs = TermBuffers::default();
    let mut buffer = make_buf(2, 20, &[], &[], 0);
    buffer.parser.process(b"\x1b[?2004h");
    bufs.lock()
        .unwrap()
        .insert("tab".into(), Arc::new(Mutex::new(buffer)));

    assert!(terminal_uses_bracketed_paste(&bufs, "tab"));
    assert!(!terminal_uses_bracketed_paste(&bufs, "missing"));

    let buffer = term_buf(&bufs, "tab").unwrap();
    buffer.lock().unwrap().parser.process(b"\x1b[?2004l");
    assert!(!terminal_uses_bracketed_paste(&bufs, "tab"));
}

#[test]
fn bash_readline_history_repaints_the_current_line() {
    let mut buffer = make_buf(4, 40, &[], &[], 0);
    let _ = buffer.ingest(b"\x1b[?2004hP> echo second");
    // GNU readline replaces "second" with the shorter "first" using six
    // backspaces, DCH for the leftover cell, then the replacement suffix.
    let _ = buffer.ingest(b"\x08\x08\x08\x08\x08\x08\x1b[1Pfirst");
    buffer.render();

    assert_eq!(buffer.displayed_text[0], "P> echo first");
    assert_eq!(buffer.parser.screen().cursor_position(), (0, 13));
}

#[test]
fn terminal_queries_reply_at_the_current_cursor_position() {
    let mut buffer = make_buf(4, 40, &[], &[], 0);

    assert_eq!(buffer.ingest(b"abc\x1b[6n"), b"\x1b[1;4R");
    assert_eq!(buffer.ingest(b"\x1b[?6n"), b"\x1b[?1;4R");
    assert_eq!(
        buffer.ingest(b"\x1b[5n\x1b[c\x1b[0c"),
        b"\x1b[0n\x1b[?1;2c\x1b[?1;2c"
    );
    assert_eq!(buffer.raw.iter().copied().collect::<Vec<_>>(), b"abc");
}

#[test]
fn terminal_query_and_hvp_scanners_survive_split_output_chunks() {
    let mut buffer = make_buf(4, 40, &[], &[], 0);

    assert!(buffer.ingest(b"\x1b[").is_empty());
    assert!(buffer.ingest(b"6").is_empty());
    assert_eq!(buffer.ingest(b"n"), b"\x1b[1;1R");

    assert!(buffer.ingest(b"\x1b[2;").is_empty());
    assert!(buffer.ingest(b"3fX").is_empty());
    assert_eq!(buffer.parser.screen().cursor_position(), (1, 3));
}

#[test]
fn ansi_save_restore_keeps_btop_message_updates_at_the_saved_cursor() {
    let mut buffer = make_buf(5, 20, &[], &[], 0);

    // btop's message box uses ANSI.SYS SCP/RCP (CSI s/u). Split SCP across
    // reads as SSH is free to divide an escape sequence.
    let _ = buffer.ingest(b"\x1b[2;3H\x1b[");
    let _ = buffer.ingest(b"s\x1b[4;6HX\x1b[uY");
    buffer.render();

    assert_eq!(buffer.displayed_text[1], "  Y");
    assert_eq!(buffer.displayed_text[3], "     X");
    assert_eq!(buffer.parser.screen().cursor_position(), (1, 3));
}

#[test]
fn synchronized_output_waits_for_a_complete_btop_frame() {
    let mut buffer = make_buf(4, 20, &[], &[], 0);
    let _ = buffer.ingest(b"stable");
    buffer.render();

    assert!(buffer.ingest(b"\x1b[?202").is_empty());
    let _ = buffer.ingest(b"6h\x1b[2;3Hframe");
    assert!(buffer.synchronized_output);
    assert!(buffer.synchronized_output_started_at.is_some());
    assert_eq!(buffer.displayed_text[0], "stable");

    let _ = buffer.ingest(b"\x1b[?2026");
    assert!(buffer.synchronized_output);
    let _ = buffer.ingest(b"l");
    assert!(!buffer.synchronized_output);
    assert!(buffer.synchronized_output_started_at.is_none());

    buffer.render();
    assert_eq!(buffer.displayed_text[0], "stable");
    assert_eq!(buffer.displayed_text[1], "  frame");
}

#[test]
fn parser_reset_discards_incomplete_synchronized_frame_state() {
    let mut buffer = make_buf(4, 20, &[], &[], 0);
    let _ = buffer.ingest(b"\x1b[?2026hframe\x1b[");

    assert!(buffer.synchronized_output);
    assert!(matches!(buffer.csi_state, CsiState::Csi));
    buffer.reset_parser(4, 20);

    assert!(!buffer.synchronized_output);
    assert!(matches!(buffer.csi_state, CsiState::Normal));
    assert!(buffer.csi_pending.is_empty());
    let _ = buffer.ingest(b"fresh");
    buffer.render();
    assert_eq!(buffer.displayed_text[0], "fresh");
}

const LEGACY_BTOP_PREAMBLE: &[u8] =
    b"\x1b[?1049h\x1b[?25l\x1b[?1002h\x1b[?1015h\x1b[?1006h\x1b[?2026h";

fn ingest_one_byte_at_a_time(buffer: &mut TermBuffer, bytes: &[u8]) {
    for byte in bytes {
        let _ = buffer.ingest(std::slice::from_ref(byte));
    }
}

fn activate_legacy_btop_compat(buffer: &mut TermBuffer) {
    ingest_one_byte_at_a_time(buffer, LEGACY_BTOP_PREAMBLE);
    ingest_one_byte_at_a_time(buffer, b"\x1b[1;1fPid: Program:");
    assert!(buffer.legacy_btop_compat);
}

#[test]
fn legacy_btop_process_control_bytes_do_not_scroll_the_alt_screen() {
    let mut buffer = make_buf(4, 80, &[], &[], 0);

    // Exercise every CSI boundary in the startup fingerprint, then split both
    // CR/LF pairs exactly as independent SSH reads can do in the real stream.
    ingest_one_byte_at_a_time(&mut buffer, LEGACY_BTOP_PREAMBLE);
    assert!(buffer.legacy_btop_candidate);
    assert!(!buffer.legacy_btop_compat);
    assert!(buffer.synchronized_output);

    ingest_one_byte_at_a_time(&mut buffer, b"\x1b[1;1fPid: Program:");
    assert!(buffer.legacy_btop_compat);
    let _ = buffer.ingest(b"\x1b[2K\x1b[1;1fTOP\x1b[3;1fbash -lc\r");
    let _ = buffer.ingest(b"\n    set -eo pipefail\r");
    let _ = buffer.ingest(b"\n    exec btop\x1b[?2026l");
    buffer.render();

    assert_eq!(buffer.displayed_text[0], "TOP");
    assert!(buffer.displayed_text[2].starts_with("bash -lc      set -eo pipefail"));
    assert_eq!(buffer.sanitized_btop_control_bytes, 4);
    assert!(!buffer.synchronized_output);
}

#[test]
fn synchronized_output_from_unrelated_tuis_preserves_crlf() {
    let mut buffer = make_buf(4, 40, &[], &[], 0);

    let _ = buffer.ingest(b"\x1b[?1049h\x1b[?2026h\x1b[1;1fone\r");
    let _ = buffer.ingest(b"\ntwo\x1b[?2026l");
    buffer.render();

    assert!(!buffer.legacy_btop_compat);
    assert_eq!(buffer.sanitized_btop_control_bytes, 0);
    assert_eq!(buffer.displayed_text[0], "one");
    assert_eq!(buffer.displayed_text[1], "two");
    let raw: Vec<u8> = buffer.raw.iter().copied().collect();
    assert!(raw.windows(2).any(|window| window == b"\r\n"));
}

#[test]
fn btop_control_preamble_without_process_header_preserves_crlf() {
    let mut buffer = make_buf(4, 40, &[], &[], 0);
    ingest_one_byte_at_a_time(&mut buffer, LEGACY_BTOP_PREAMBLE);
    let _ = buffer.ingest(b"\x1b[1;1fother app\r\ntwo\x1b[?2026l");
    buffer.render();

    assert!(buffer.legacy_btop_candidate);
    assert!(!buffer.legacy_btop_compat);
    assert_eq!(buffer.sanitized_btop_control_bytes, 0);
    assert_eq!(buffer.displayed_text[0], "other app");
    assert_eq!(buffer.displayed_text[1], "two");
}

#[test]
fn leaving_legacy_btop_restores_normal_crlf_semantics() {
    let mut buffer = make_buf(4, 40, &[], &[], 0);
    ingest_one_byte_at_a_time(&mut buffer, LEGACY_BTOP_PREAMBLE);
    let _ = buffer.ingest(b"\x1b[1;1fPid: Program:\x1b[2;1fargv\r\n\x1b[?2026l\x1b[?1049l");

    assert!(!buffer.legacy_btop_compat);
    let sanitized = buffer.sanitized_btop_control_bytes;
    let _ = buffer.ingest(b"shell\r\nnext");
    buffer.render();

    assert_eq!(buffer.sanitized_btop_control_bytes, sanitized);
    assert_eq!(buffer.displayed_text[0], "shell");
    assert_eq!(buffer.displayed_text[1], "next");
}

#[test]
fn legacy_btop_compat_preserves_non_crlf_controls_and_lone_breaks() {
    let mut buffer = make_buf(4, 80, &[], &[], 0);
    activate_legacy_btop_compat(&mut buffer);

    let _ = buffer.ingest(b"\x1b[2;1fA\tB\x07C\nD\rX");

    assert_eq!(buffer.sanitized_btop_control_bytes, 0);
    assert!(!buffer.legacy_btop_pending_cr);
    let raw: Vec<u8> = buffer.raw.iter().copied().collect();
    assert!(raw.ends_with(b"\x1b[2;1HA\tB\x07C\nD\rX"));
}

#[test]
fn aborting_an_incomplete_sync_frame_flushes_pending_cr_and_allows_render() {
    let mut buffer = make_buf(4, 80, &[], &[], 0);
    activate_legacy_btop_compat(&mut buffer);
    let _ = buffer.ingest(b"\x1b[2;1fpartial\r");

    assert!(buffer.synchronized_output);
    assert!(buffer.legacy_btop_pending_cr);
    buffer.abort_synchronized_output();
    buffer.render();

    assert!(!buffer.synchronized_output);
    assert!(!buffer.legacy_btop_pending_cr);
    assert_eq!(buffer.sanitized_btop_control_bytes, 0);
    assert_eq!(buffer.displayed_text[1], "partial");
    assert_eq!(buffer.raw.back(), Some(&b'\r'));
}

#[test]
fn legacy_btop_tree_header_enables_the_same_crlf_guard() {
    let mut buffer = make_buf(4, 80, &[], &[], 0);
    ingest_one_byte_at_a_time(&mut buffer, LEGACY_BTOP_PREAMBLE);
    ingest_one_byte_at_a_time(&mut buffer, b"\x1b[1;1fTree:");
    let _ = buffer.ingest(b"\x1b[2;1fargv\r\ncontinues\x1b[?2026l");

    assert!(buffer.legacy_btop_compat);
    assert_eq!(buffer.sanitized_btop_control_bytes, 2);
}

#[test]
fn btop_header_probe_cannot_span_cursor_moves_or_sync_frames() {
    let mut buffer = make_buf(4, 80, &[], &[], 0);
    ingest_one_byte_at_a_time(&mut buffer, LEGACY_BTOP_PREAMBLE);
    let _ = buffer.ingest(b"\x1b[1;1fPid: \x1b[2;1fProgram:");
    assert!(!buffer.legacy_btop_compat);

    let _ = buffer.ingest(b"\x1b[?2026l\x1b[?2026h\x1b[1;1fPid: ");
    let _ = buffer.ingest(b"\x1b[?2026l\x1b[?2026h\x1b[1;1fProgram:");
    assert!(!buffer.legacy_btop_compat);
}

#[test]
fn canceled_csi_does_not_hide_a_later_synchronized_output_terminator() {
    for cancel in [b"\x1b".as_slice(), b"\x18".as_slice(), b"\x1a".as_slice()] {
        let mut buffer = make_buf(4, 80, &[], &[], 0);
        let _ = buffer.ingest(b"\x1b[?2026hframe\x1b[12");
        assert!(matches!(buffer.csi_state, CsiState::Csi));
        let _ = buffer.ingest(cancel);
        let _ = buffer.ingest(b"\x1b[?2026l");

        assert!(!buffer.synchronized_output, "cancel byte {cancel:?}");
        assert!(matches!(buffer.csi_state, CsiState::Normal));
        assert!(buffer.csi_pending.is_empty());
    }
}

#[test]
fn aborting_synchronized_output_discards_a_partial_csi() {
    let mut buffer = make_buf(4, 80, &[], &[], 0);
    let _ = buffer.ingest(b"\x1b[?2026hvisible\x1b[");
    assert!(matches!(buffer.csi_state, CsiState::Csi));

    buffer.abort_synchronized_output();
    let _ = buffer.ingest(b"\r\nnotice");
    buffer.render();

    assert!(matches!(buffer.csi_state, CsiState::Normal));
    assert!(buffer.csi_pending.is_empty());
    assert!(buffer.displayed_text.iter().any(|line| line == "notice"));
}

#[test]
fn csi_3j_clears_meatshell_scrollback_even_when_split() {
    let mut buffer = make_buf(3, 20, &["old one", "old two"], &["current"], 2);
    buffer.raw.extend(b"old one\nold two\n");
    buffer.prev.push(hist_line("old two"));
    buffer.sel_anchor = Some((0, 0));
    buffer.sel_focus = Some((1, 2));

    let _ = buffer.ingest(b"\x1b[3");
    assert_eq!(buffer.history.len(), 2);
    let _ = buffer.ingest(b"J");

    assert!(buffer.history.is_empty());
    assert_eq!(buffer.view_offset, 0);
    assert!(buffer.raw.is_empty());
    assert!(buffer.sel_anchor.is_none());
    assert!(buffer.sel_focus.is_none());
}

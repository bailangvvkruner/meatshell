#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TerminalMouseEventKind {
    Press,
    Release,
    Motion,
}

fn append_utf8_value(output: &mut Vec<u8>, value: u16) {
    let mut encoded = [0u8; 4];
    let character =
        char::from_u32(u32::from(value)).expect("mouse protocol value is valid Unicode");
    output.extend_from_slice(character.encode_utf8(&mut encoded).as_bytes());
}

/// Encode one xterm mouse report. `Some([])` means tracking is active, but the
/// selected mode intentionally suppresses this event (for example X10 release).
pub(crate) fn encode_terminal_mouse_event(
    screen: &vt100::Screen,
    kind: TerminalMouseEventKind,
    button: u16,
    col: i32,
    row: i32,
    ctrl: bool,
    alt: bool,
) -> Option<Vec<u8>> {
    use vt100::{MouseProtocolEncoding, MouseProtocolMode};

    let mode = screen.mouse_protocol_mode();
    if mode == MouseProtocolMode::None {
        return None;
    }

    let modifiers = u16::from(alt) * 8 + u16::from(ctrl) * 16;
    let (button_code, release) = match kind {
        TerminalMouseEventKind::Press => (button + modifiers, false),
        TerminalMouseEventKind::Release => {
            if mode == MouseProtocolMode::Press {
                return Some(Vec::new());
            }
            let code = if screen.mouse_protocol_encoding() == MouseProtocolEncoding::Sgr {
                button + modifiers
            } else {
                3 + modifiers
            };
            (code, true)
        }
        TerminalMouseEventKind::Motion => {
            let reports_motion = match mode {
                MouseProtocolMode::ButtonMotion => button <= 2,
                MouseProtocolMode::AnyMotion => true,
                _ => false,
            };
            if !reports_motion {
                return Some(Vec::new());
            }
            (button.min(3) + 32 + modifiers, false)
        }
    };

    let (rows, cols) = screen.size();
    let col = (col.clamp(0, cols.saturating_sub(1) as i32) as u16) + 1;
    let row = (row.clamp(0, rows.saturating_sub(1) as i32) as u16) + 1;

    match screen.mouse_protocol_encoding() {
        MouseProtocolEncoding::Sgr => Some(
            format!(
                "\x1b[<{button_code};{col};{row}{}",
                if release { 'm' } else { 'M' }
            )
            .into_bytes(),
        ),
        MouseProtocolEncoding::Default => Some(vec![
            0x1b,
            b'[',
            b'M',
            (button_code + 32).min(255) as u8,
            (col.min(223) + 32) as u8,
            (row.min(223) + 32) as u8,
        ]),
        MouseProtocolEncoding::Utf8 => {
            let mut output = b"\x1b[M".to_vec();
            append_utf8_value(&mut output, (button_code + 32).min(2047));
            append_utf8_value(&mut output, col.min(2015) + 32);
            append_utf8_value(&mut output, row.min(2015) + 32);
            Some(output)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{encode_terminal_mouse_event, TerminalMouseEventKind};

    fn parser_with_mode(mode: &[u8]) -> vt100::Parser {
        let mut parser = vt100::Parser::new(24, 80, 0);
        parser.process(mode);
        parser
    }

    #[test]
    fn tracking_off_leaves_clicks_for_local_selection() {
        let parser = parser_with_mode(b"");
        assert_eq!(
            encode_terminal_mouse_event(
                parser.screen(),
                TerminalMouseEventKind::Press,
                0,
                4,
                2,
                false,
                false,
            ),
            None
        );
    }

    #[test]
    fn sgr_double_click_reports_two_complete_clicks() {
        let parser = parser_with_mode(b"\x1b[?1000h\x1b[?1006h");
        let report = |kind| {
            encode_terminal_mouse_event(parser.screen(), kind, 0, 4, 2, false, false).unwrap()
        };
        let actual = [
            report(TerminalMouseEventKind::Press),
            report(TerminalMouseEventKind::Release),
            report(TerminalMouseEventKind::Press),
            report(TerminalMouseEventKind::Release),
        ]
        .concat();
        assert_eq!(actual, b"\x1b[<0;5;3M\x1b[<0;5;3m\x1b[<0;5;3M\x1b[<0;5;3m");
    }

    #[test]
    fn x10_consumes_release_without_reporting_it() {
        let parser = parser_with_mode(b"\x1b[?9h");
        assert_eq!(
            encode_terminal_mouse_event(
                parser.screen(),
                TerminalMouseEventKind::Press,
                0,
                0,
                0,
                false,
                false,
            ),
            Some(vec![0x1b, b'[', b'M', 32, 33, 33])
        );
        assert_eq!(
            encode_terminal_mouse_event(
                parser.screen(),
                TerminalMouseEventKind::Release,
                0,
                0,
                0,
                false,
                false,
            ),
            Some(Vec::new())
        );
    }

    #[test]
    fn button_motion_requires_a_pressed_button() {
        let parser = parser_with_mode(b"\x1b[?1002h\x1b[?1006h");
        assert_eq!(
            encode_terminal_mouse_event(
                parser.screen(),
                TerminalMouseEventKind::Motion,
                3,
                7,
                8,
                false,
                false,
            ),
            Some(Vec::new())
        );
        assert_eq!(
            encode_terminal_mouse_event(
                parser.screen(),
                TerminalMouseEventKind::Motion,
                0,
                7,
                8,
                false,
                false,
            ),
            Some(b"\x1b[<32;8;9M".to_vec())
        );
    }

    #[test]
    fn any_motion_reports_without_a_pressed_button() {
        let parser = parser_with_mode(b"\x1b[?1003h\x1b[?1006h");
        assert_eq!(
            encode_terminal_mouse_event(
                parser.screen(),
                TerminalMouseEventKind::Motion,
                3,
                7,
                8,
                false,
                false,
            ),
            Some(b"\x1b[<35;8;9M".to_vec())
        );
    }

    #[test]
    fn utf8_encoding_preserves_coordinates_above_223() {
        let mut parser = vt100::Parser::new(24, 400, 0);
        parser.process(b"\x1b[?1000h\x1b[?1005h");
        assert_eq!(
            encode_terminal_mouse_event(
                parser.screen(),
                TerminalMouseEventKind::Press,
                0,
                250,
                2,
                false,
                false,
            ),
            Some(vec![0x1b, b'[', b'M', b' ', 0xc4, 0x9b, b'#'])
        );
    }
}

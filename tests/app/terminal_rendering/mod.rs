use super::*;

fn hist_line(s: &str) -> Line {
    (s.to_string(), Vec::new(), false)
}

fn wrapped_hist_line(s: &str) -> Line {
    (s.to_string(), Vec::new(), true)
}

fn make_buf(
    rows: u16,
    cols: u16,
    history: &[&str],
    live_lines: &[&str],
    view_offset: usize,
) -> TermBuffer {
    let mut parser = vt100::Parser::new(rows, cols, 0);
    parser.process(live_lines.join("\r\n").as_bytes());
    TermBuffer {
        parser,
        ui_cols: cols as u32,
        ui_rows: rows as u32,
        requested_pty_cols: cols as u32,
        requested_pty_rows: rows as u32,
        raster_cell_width: 8.0,
        raster_cell_height: 16.0,
        find_query: String::new(),
        is_dark: false,
        output_highlight: OutputHighlightPreset::Log,
        custom_highlight_rules: Vec::new(),
        sel_anchor: None,
        sel_focus: None,
        sel_ranges: Vec::new(),
        history: history.iter().map(|s| hist_line(s)).collect(),
        prev: Vec::new(),
        view_offset,
        displayed_text: Vec::new(),
        synchronized_output: false,
        synchronized_output_started_at: None,
        synchronized_output_timeout_scheduled: false,
        legacy_btop_probe: 0,
        legacy_btop_candidate: false,
        legacy_btop_text_probe: 0,
        legacy_btop_tree_probe: 0,
        legacy_btop_compat: false,
        sync_frame_has_hvp: false,
        legacy_btop_pending_cr: false,
        sanitized_btop_control_bytes: 0,
        csi_state: CsiState::Normal,
        csi_pending: Vec::new(),
        raw: std::collections::VecDeque::new(),
    }
}

mod colors;
mod protocol;
mod selection;
mod sftp_sorting;

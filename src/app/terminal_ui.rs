use super::*;

pub(super) fn history_model(store: &ConfigStore) -> ModelRc<SharedString> {
    let rows: Vec<SharedString> = store
        .command_history()
        .iter()
        .map(|s| s.clone().into())
        .collect();
    ModelRc::from(Rc::new(VecModel::from(rows)))
}

pub(super) fn output_highlight_rule_model(store: &ConfigStore) -> ModelRc<OutputRuleItem> {
    let rows: Vec<OutputRuleItem> = store
        .output_highlight_rules()
        .iter()
        .map(|rule| OutputRuleItem {
            pattern: rule.pattern.clone().into(),
            regex: rule.regex,
            case_sensitive: rule.case_sensitive,
            whole_line: rule.whole_line,
            color: match rule.color.as_str() {
                "yellow" | "green" | "cyan" | "magenta" | "gray" => rule.color.clone(),
                _ => "red".to_string(),
            }
            .into(),
            enabled: rule.enabled,
        })
        .collect();
    ModelRc::from(Rc::new(VecModel::from(rows)))
}

pub(super) fn parse_hex_color(value: &str) -> Option<slint::Color> {
    let digits = value.trim().strip_prefix('#').unwrap_or(value.trim());
    if digits.len() != 6 || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let red = u8::from_str_radix(&digits[0..2], 16).ok()?;
    let green = u8::from_str_radix(&digits[2..4], 16).ok()?;
    let blue = u8::from_str_radix(&digits[4..6], 16).ok()?;
    Some(slint::Color::from_rgb_u8(red, green, blue))
}

pub(super) fn validate_output_highlight_rule(
    pattern: &str,
    is_regex: bool,
    case_sensitive: bool,
) -> std::result::Result<(), String> {
    if pattern.is_empty() {
        return Err(t(
            "请输入关键词或正则表达式",
            "Enter a keyword or regular expression",
        )
        .into());
    }
    if pattern.chars().count() > 512 {
        return Err(t(
            "规则不能超过 512 个字符",
            "Rules cannot exceed 512 characters",
        )
        .into());
    }
    if is_regex {
        regex::RegexBuilder::new(pattern)
            .case_insensitive(!case_sensitive)
            .build()
            .map_err(|error| {
                format!(
                    "{}: {error}",
                    t("无效的正则表达式", "Invalid regular expression")
                )
            })?;
    }
    Ok(())
}

/// Build the filtered history-view rows for the dropdown, newest first. The
/// command-history model itself remains oldest first so ↑/↓ recall keeps its
/// existing shell-like navigation semantics (#55, #101, #331).
pub(super) fn history_view_rows(history: &[String], query: &str) -> Vec<SharedString> {
    let q = query.trim().to_lowercase();
    history
        .iter()
        .rev()
        .filter(|command| q.is_empty() || command.to_lowercase().contains(&q))
        .map(|command| command.clone().into())
        .collect()
}

/// Build the filtered history-view model for the dropdown: case-insensitive
/// substring matches of `query`, ordered from newest to oldest (#101, #331).
pub(super) fn history_view_model(store: &ConfigStore, query: &str) -> ModelRc<SharedString> {
    let rows = history_view_rows(store.command_history(), query);
    ModelRc::from(Rc::new(VecModel::from(rows)))
}

#[cfg(test)]
#[path = "../../tests/app/command_history/mod.rs"]
mod history_view_tests;

#[cfg(test)]
#[path = "../../tests/app/terminal_rendering/model_sync/mod.rs"]
mod model_sync_tests;

/// Find every (case-insensitive) occurrence of `query` across the currently
/// displayed rows and return highlight rectangles in GRID-COLUMN space (wide
/// CJK glyphs count as two columns, so highlights line up over the text #132).
pub(super) fn compute_find_matches(rows: &[String], query: &str) -> Vec<TermMatch> {
    let mut out: Vec<TermMatch> = Vec::new();
    if query.is_empty() {
        return out;
    }
    let q: Vec<char> = query.chars().map(|c| c.to_ascii_lowercase()).collect();
    if q.is_empty() {
        return out;
    }
    for (r, line) in rows.iter().enumerate() {
        let chars: Vec<char> = line.chars().collect();
        let lower: Vec<char> = chars.iter().map(|c| c.to_ascii_lowercase()).collect();
        let prefix = cell_prefix(&chars);
        let mut i = 0usize;
        while i + q.len() <= lower.len() {
            if lower[i..i + q.len()] == q[..] {
                let col = prefix[i] as i32;
                let len = (prefix[i + q.len()] - prefix[i]) as i32;
                out.push(TermMatch {
                    row: r as i32,
                    col,
                    len,
                });
                i += q.len();
            } else {
                i += 1;
            }
        }
    }
    out
}

/// Apply a settled terminal size to the PTY + vt100 grid. Factored out of the
/// resize callback so that callback can debounce — a layout reflow can briefly
/// report a near-zero width, collapsing term-cols to its 10-col floor; applying
/// that to the remote PTY reflows vt100 and garbles running output like a
/// `git clone` progress meter (#163). Debouncing means only the settled size
/// ever reaches the server.
pub(super) fn apply_terminal_resize(
    handles: &Rc<RefCell<HashMap<String, SessionHandle>>>,
    bufs: &TermBuffers,
    last_term_size: &Arc<Mutex<(u32, u32)>>,
    tab_id: &str,
    cols: u32,
    rows: u32,
) {
    *last_term_size.lock().unwrap() = (cols, rows);
    if let Some(handle) = handles.borrow().get(tab_id) {
        handle.resize(cols, rows);
    }
    if let Some(h) = term_buf(bufs, tab_id) {
        let mut buf = h.lock().unwrap();
        let (old_rows, old_cols) = buf.parser.screen().size();
        let (new_rows, new_cols) = (rows as u16, cols as u16);
        if (new_rows, new_cols) != (old_rows, old_cols) {
            if buf.parser.screen().alternate_screen() {
                // Alt-screen (tmux/vim/btop): the remote redraws the whole screen
                // on SIGWINCH, so just resize the grid and let that redraw fill it.
                buf.parser.set_size(new_rows, new_cols);
            } else {
                // Reflow already-printed output to the new width by replaying the
                // byte stream — vt100's set_size only truncates/pads (#169).
                buf.reflow(new_rows, new_cols);
            }
            // The pre/post-resize screens differ; drop the scroll-detection
            // snapshot so the next output isn't mis-read as a scroll.
            buf.prev.clear();
        }
    }
}

/// Recompute spans + cursor + find/selection highlights for one tab from its
/// current vt100 screen (respecting scrollback) and push them to the model.
/// Used by scroll + selection callbacks (Output has its own equivalent inline).
fn term_span_eq(a: &TermSpan, b: &TermSpan) -> bool {
    a.text == b.text
        && a.fg == b.fg
        && a.bg == b.bg
        && a.bold == b.bold
        && a.row == b.row
        && a.col == b.col
        && a.cells == b.cells
        && a.cjk == b.cjk
        && a.emoji == b.emoji
        && (!a.emoji || a.emoji_image == b.emoji_image)
}

pub(super) fn term_match_eq(a: &TermMatch, b: &TermMatch) -> bool {
    a.row == b.row && a.col == b.col && a.len == b.len
}

fn term_span_key(span: &TermSpan) -> (i32, i32) {
    (span.row, span.col)
}

/// Synchronize row-major spans by grid position. A style change can split or
/// merge one run; index-only alignment would rewrite every later span.
pub(super) fn sync_term_span_rows(model: &ModelRc<TermSpan>, rows: &[TermSpan]) -> bool {
    let model = model
        .as_any()
        .downcast_ref::<VecModel<TermSpan>>()
        .expect("terminal display model must be a VecModel");
    let mut model_index = 0;
    let mut row_index = 0;
    let mut changed = false;

    while model_index < model.row_count() && row_index < rows.len() {
        let current = model
            .row_data(model_index)
            .expect("terminal span index must be in bounds");
        match term_span_key(&current).cmp(&term_span_key(&rows[row_index])) {
            std::cmp::Ordering::Less => {
                model.remove(model_index);
                changed = true;
            }
            std::cmp::Ordering::Greater => {
                model.insert(model_index, rows[row_index].clone());
                model_index += 1;
                row_index += 1;
                changed = true;
            }
            std::cmp::Ordering::Equal => {
                if !term_span_eq(&current, &rows[row_index]) {
                    model.set_row_data(model_index, rows[row_index].clone());
                    changed = true;
                }
                model_index += 1;
                row_index += 1;
            }
        }
    }

    while model_index < model.row_count() {
        model.remove(model_index);
        changed = true;
    }
    if row_index < rows.len() {
        model.extend_from_slice(&rows[row_index..]);
        changed = true;
    }

    changed
}

/// Keep the same Slint model so repeaters retain their components, notifying
/// only changed rows and any added or removed tail.
pub(super) fn sync_vec_model_rows<T: Clone + 'static>(
    model: &ModelRc<T>,
    rows: &[T],
    rows_equal: fn(&T, &T) -> bool,
) -> bool {
    let model = model
        .as_any()
        .downcast_ref::<VecModel<T>>()
        .expect("terminal display model must be a VecModel");
    let old_len = model.row_count();
    let new_len = rows.len();
    let common = old_len.min(new_len);
    let mut changed = false;

    for (index, next) in rows.iter().take(common).enumerate() {
        let differs = model
            .row_data(index)
            .map(|current| !rows_equal(&current, next))
            .unwrap_or(true);
        if differs {
            model.set_row_data(index, next.clone());
            changed = true;
        }
    }

    if new_len < old_len {
        for index in (new_len..old_len).rev() {
            model.remove(index);
        }
        changed = true;
    } else if new_len > old_len {
        model.extend_from_slice(&rows[old_len..]);
        changed = true;
    }

    changed
}

pub(super) fn terminal_row_images_enabled() -> bool {
    static OVERRIDE: OnceLock<Option<bool>> = OnceLock::new();
    match *OVERRIDE.get_or_init(|| match std::env::var("MEATSHELL_TERMINAL_RENDERER") {
        Ok(value) if matches!(value.trim().to_ascii_lowercase().as_str(), "spans" | "text") => {
            Some(false)
        }
        Ok(value)
            if matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "rows" | "images"
            ) =>
        {
            Some(true)
        }
        _ => None,
    }) {
        Some(enabled) => enabled,
        None => cfg!(windows) && active_renderer_uses_gpu(),
    }
}

fn terminal_frame_prefers_row_images(
    is_alt: bool,
    span_count: usize,
    rows: u16,
    contains_emoji: bool,
) -> bool {
    !contains_emoji && (is_alt || span_count >= (rows as usize).saturating_mul(3).max(96))
}

fn terminal_rasterizer() -> &'static TerminalRasterizer {
    static RASTERIZER: OnceLock<TerminalRasterizer> = OnceLock::new();
    RASTERIZER.get_or_init(TerminalRasterizer::new)
}

fn raster_color(color: slint::Color) -> [u8; 4] {
    [color.red(), color.green(), color.blue(), color.alpha()]
}

fn raster_rows(spans: &[TermSpan], row_count: usize) -> Vec<Vec<RasterSpan>> {
    let mut rows = vec![Vec::new(); row_count];
    for span in spans {
        let Ok(row) = usize::try_from(span.row) else {
            continue;
        };
        let Some(output) = rows.get_mut(row) else {
            continue;
        };
        output.push(RasterSpan {
            text: span.text.to_string(),
            foreground: raster_color(span.fg),
            background: raster_color(span.bg),
            bold: span.bold,
            col: span.col.max(0) as u32,
            cells: span.cells.max(0) as u32,
            cjk: span.cjk,
        });
    }
    rows
}

#[derive(Default)]
struct RasterUiState {
    pending: HashMap<String, RasterCompletion>,
    scheduled: HashSet<String>,
    requested: HashSet<String>,
}

fn raster_ui_state() -> &'static Mutex<RasterUiState> {
    static STATE: OnceLock<Mutex<RasterUiState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(RasterUiState::default()))
}

fn set_terminal_raster_requested(tab_id: &str, requested: bool) {
    let mut state = raster_ui_state().lock().unwrap();
    if requested {
        state.requested.insert(tab_id.to_string());
    } else {
        state.requested.remove(tab_id);
    }
}

fn terminal_raster_requested(tab_id: &str) -> bool {
    raster_ui_state().lock().unwrap().requested.contains(tab_id)
}

fn merge_raster_completion(slot: &mut RasterCompletion, incoming: RasterCompletion) {
    match (slot, incoming) {
        (Ok(current), Ok(next)) => current.merge_from(next),
        (slot, incoming) => *slot = incoming,
    }
}

fn forget_terminal_raster_ui(tab_id: &str) {
    let mut state = raster_ui_state().lock().unwrap();
    state.pending.remove(tab_id);
    state.scheduled.remove(tab_id);
    state.requested.remove(tab_id);
}

pub(super) fn forget_terminal_raster(tab_id: &str) {
    terminal_rasterizer().forget(tab_id);
    forget_terminal_raster_ui(tab_id);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RasterApplyAction {
    Drop,
    Cache,
    Publish,
}

fn raster_apply_action(requested: bool, state: RasterResultState) -> RasterApplyAction {
    match (requested, state) {
        (true, RasterResultState::Latest) => RasterApplyAction::Publish,
        (true, RasterResultState::Stale) => RasterApplyAction::Cache,
        _ => RasterApplyAction::Drop,
    }
}

fn recycle_terminal_row_image(image: TermRowImage) {
    if image.valid {
        if let Some(buffer) = image.source.to_rgba8_premultiplied() {
            recycle_pixel_buffer(buffer);
        }
    }
}

fn recycle_terminal_raster_result(result: RasterResult) {
    for update in result.updates {
        recycle_pixel_buffer(update.pixels);
    }
}

pub(super) fn clear_terminal_row_images(model: &VecModel<TermRowImage>) {
    while model.row_count() > 0 {
        let index = model.row_count() - 1;
        if let Some(image) = model.row_data(index) {
            recycle_terminal_row_image(image);
        }
        model.remove(index);
    }
}

fn apply_terminal_raster_result(win: &AppWindow, mut result: RasterResult, is_latest: bool) {
    let tab_id = result.tab_id.clone();
    let changed_rows = result.updates.len();
    let mut updates = Some(std::mem::take(&mut result.updates));
    set_terminal_row(win, &tab_id, |row| {
        let model = row
            .row_images
            .as_any()
            .downcast_ref::<VecModel<TermRowImage>>()
            .expect("terminal row image model must be a VecModel");

        let columns = result.columns.min(i32::MAX as u32) as i32;
        let geometry_changed = row.row_image_columns != columns
            || (row.row_image_cell_width - result.logical_cell_width).abs() > f32::EPSILON
            || (row.row_image_cell_height - result.logical_cell_height).abs() > f32::EPSILON;
        if geometry_changed {
            for index in 0..model.row_count() {
                if let Some(image) = model.row_data(index) {
                    recycle_terminal_row_image(image);
                }
                model.set_row_data(
                    index,
                    TermRowImage {
                        source: Image::default(),
                        valid: false,
                    },
                );
            }
        }

        while model.row_count() > result.row_count {
            let index = model.row_count() - 1;
            if let Some(image) = model.row_data(index) {
                recycle_terminal_row_image(image);
            }
            model.remove(index);
        }
        while model.row_count() < result.row_count {
            model.push(TermRowImage {
                source: Image::default(),
                valid: false,
            });
        }
        for update in updates.take().unwrap_or_default() {
            if update.row >= result.row_count
                || update.pixels.width() != update.width
                || update.pixels.height() != update.height
            {
                recycle_pixel_buffer(update.pixels);
                continue;
            }
            let previous = model.row_data(update.row);
            model.set_row_data(
                update.row,
                TermRowImage {
                    source: Image::from_rgba8_premultiplied(update.pixels),
                    valid: true,
                },
            );
            if let Some(previous) = previous {
                recycle_terminal_row_image(previous);
            }
        }
        let all_rows_valid = result.row_count > 0
            && (0..result.row_count)
                .all(|index| model.row_data(index).is_some_and(|image| image.valid));
        row.row_image_columns = columns;
        row.row_image_cell_width = result.logical_cell_width;
        row.row_image_cell_height = result.logical_cell_height;
        row.row_images_ready = is_latest && all_rows_valid && result.has_content;
        if is_latest && all_rows_valid {
            row.cursor_row = result.cursor_row;
            row.cursor_col = result.cursor_col;
        }
    });
    tracing::trace!(
        tab_id,
        generation = result.generation,
        is_latest,
        rows = result.row_count,
        changed_rows,
        "terminal row raster applied"
    );
    win.window().request_redraw();
}

fn apply_terminal_raster_completion(win: &AppWindow, tab_id: &str, completion: RasterCompletion) {
    match completion {
        Ok(result) => {
            let state = terminal_rasterizer().result_state(&result);
            match raster_apply_action(terminal_raster_requested(&result.tab_id), state) {
                RasterApplyAction::Drop => recycle_terminal_raster_result(result),
                RasterApplyAction::Cache => apply_terminal_raster_result(win, result, false),
                RasterApplyAction::Publish => apply_terminal_raster_result(win, result, true),
            }
        }
        Err(error) => {
            tracing::warn!(tab_id, %error, "terminal row raster failed");
            if terminal_raster_requested(tab_id) {
                set_terminal_row(win, tab_id, |row| row.row_images_ready = false);
                win.window().request_redraw();
            }
        }
    }
}

fn schedule_terminal_raster_ui(weak: slint::Weak<AppWindow>, tab_id: String) {
    let tab_for_event = tab_id.clone();
    let invoke = slint::invoke_from_event_loop(move || {
        let Some(win) = weak.upgrade() else {
            forget_terminal_raster_ui(&tab_for_event);
            return;
        };
        loop {
            let completion = raster_ui_state()
                .lock()
                .unwrap()
                .pending
                .remove(&tab_for_event);
            if let Some(completion) = completion {
                apply_terminal_raster_completion(&win, &tab_for_event, completion);
            }
            let mut state = raster_ui_state().lock().unwrap();
            if state.pending.contains_key(&tab_for_event) {
                continue;
            }
            state.scheduled.remove(&tab_for_event);
            break;
        }
    });
    if let Err(error) = invoke {
        forget_terminal_raster_ui(&tab_id);
        tracing::debug!(%error, "terminal raster result dropped after event-loop exit");
    }
}

fn enqueue_terminal_raster_ui(
    weak: slint::Weak<AppWindow>,
    tab_id: String,
    completion: RasterCompletion,
) {
    let should_schedule = {
        let mut state = raster_ui_state().lock().unwrap();
        if let Some(pending) = state.pending.get_mut(&tab_id) {
            merge_raster_completion(pending, completion);
        } else {
            state.pending.insert(tab_id.clone(), completion);
        }
        state.scheduled.insert(tab_id.clone())
    };
    if should_schedule {
        schedule_terminal_raster_ui(weak, tab_id);
    }
}

#[allow(clippy::too_many_arguments)]
fn submit_terminal_raster(
    win: &AppWindow,
    tab_id: &str,
    spans: &[TermSpan],
    rows: u16,
    cols: u16,
    logical_cell_width: f32,
    logical_cell_height: f32,
    cursor_row: i32,
    cursor_col: i32,
) {
    if !terminal_row_images_enabled() {
        return;
    }
    let scale = win.window().scale_factor().max(0.5);
    let job = RasterJob::new(
        tab_id.to_string(),
        RasterConfig {
            columns: cols as u32,
            cell_width: logical_cell_width.max(1.0) * scale,
            cell_height: logical_cell_height.max(1.0) * scale,
            font_size: win.get_term_font_size().max(1.0) * scale,
            terminal_family: win.get_term_font_family().to_string(),
            cjk_family: win.get_ui_font_family().to_string(),
            force_bold: win.get_term_font_bold(),
        },
        raster_rows(spans, rows as usize),
    )
    .with_frame_state(
        logical_cell_width.max(1.0),
        logical_cell_height.max(1.0),
        cursor_row,
        cursor_col,
    );
    let weak = win.as_weak();
    let tab_id = tab_id.to_string();
    terminal_rasterizer().submit(job, move |completion| {
        enqueue_terminal_raster_ui(weak, tab_id, completion);
    });
}

pub(super) fn rebuild_tab_display(win: &AppWindow, bufs: &TermBuffers, tab_id: &str) {
    let data = with_term_buf(bufs, tab_id, |buf| {
        let (rows, cols) = buf.parser.screen().size();
        let b = buf.render(); // also refreshes buf.displayed_text
        let matches = compute_find_matches(&buf.displayed_text, &buf.find_query);
        let sel = buf.selection_rects_visible(cols);
        (
            b,
            matches,
            sel,
            rows,
            cols,
            buf.raster_cell_width,
            buf.raster_cell_height,
        )
    });
    let Some((b, matches, sel, rows, cols, cell_width, cell_height)) = data else {
        return;
    };
    let (cr, cc, ru, alt) = (b.cursor_row, b.cursor_col, b.rows_used, b.is_alt);
    let (smax, soff) = (b.scroll_max, b.scroll_offset);
    let contains_emoji = b.spans.iter().any(|span| span.emoji);
    let dense_frame = terminal_frame_prefers_row_images(alt, b.spans.len(), rows, contains_emoji);
    crate::memory_trim::record_terminal_activity(tab_id, dense_frame);
    let row_images_for_frame = terminal_row_images_enabled() && dense_frame;
    let mut changed = false;
    let mut forget_sparse_raster = false;
    set_terminal_row(win, tab_id, |row| {
        let span_changed = sync_term_span_rows(&row.spans, &b.spans);
        let overlay_changed = sync_vec_model_rows(&row.find_matches, &matches, term_match_eq)
            | sync_vec_model_rows(&row.selection, &sel, term_match_eq);
        let screen_mode_changed = row.is_alt_screen != alt;
        let cursor_position_changed = row.cursor_row != cr || row.cursor_col != cc;

        if !row_images_for_frame {
            if row.row_image_columns != 0 || row.row_images_ready {
                forget_sparse_raster = true;
                if let Some(model) = row
                    .row_images
                    .as_any()
                    .downcast_ref::<VecModel<TermRowImage>>()
                {
                    clear_terminal_row_images(model);
                }
                row.row_image_columns = 0;
                row.row_image_cell_width = 0.0;
                row.row_image_cell_height = 0.0;
            }
            row.row_images_ready = false;
        }
        if row_images_for_frame && !alt && (span_changed || cursor_position_changed) {
            row.row_images_ready = false;
        }
        let immediate_cursor =
            !row_images_for_frame || !row.row_images_ready || screen_mode_changed;
        let cursor_changed = immediate_cursor && cursor_position_changed;
        let scalar_changed = cursor_changed
            || row.rows_used != ru
            || row.is_alt_screen != alt
            || row.scroll_max != smax
            || row.scroll_offset != soff;
        if screen_mode_changed && row_images_for_frame {
            forget_terminal_raster(tab_id);
            row.row_images_ready = false;
        }
        if scalar_changed {
            if immediate_cursor {
                row.cursor_row = cr;
                row.cursor_col = cc;
            }
            row.rows_used = ru;
            row.is_alt_screen = alt;
            row.scroll_max = smax;
            row.scroll_offset = soff;
        }
        changed = scalar_changed || overlay_changed || (span_changed && !row.row_images_ready);
        if (span_changed || overlay_changed || scalar_changed) && !alt {
            // Normal-screen output must keep scrollback pinned. Alt-screen TUIs
            // are fixed at y=0 and nested model notifications are sufficient.
            row.render_revision = row.render_revision.wrapping_add(1);
        }
    });
    if changed {
        win.window().request_redraw();
    }
    if !row_images_for_frame {
        forget_terminal_raster(tab_id);
    }
    if forget_sparse_raster {
        clear_pixel_pool();
    }
    if row_images_for_frame {
        set_terminal_raster_requested(tab_id, true);
        submit_terminal_raster(
            win,
            tab_id,
            &b.spans,
            rows,
            cols,
            cell_width,
            cell_height,
            cr,
            cc,
        );
    }
}

pub(super) fn refresh_visible_terminal_rasters(win: &AppWindow, bufs: &TermBuffers) {
    if !terminal_row_images_enabled() {
        return;
    }
    for tab_id in visible_tab_ids(win) {
        forget_terminal_raster(&tab_id);
        set_terminal_row(win, &tab_id, |row| row.row_images_ready = false);
        rebuild_tab_display(win, bufs, &tab_id);
    }
    win.window().request_redraw();
}

pub(super) fn rebuild_all_terminal_displays(window: &AppWindow, bufs: &TermBuffers) {
    let tab_ids: Vec<String> = bufs.lock().unwrap().keys().cloned().collect();
    for tab_id in tab_ids {
        forget_terminal_raster(&tab_id);
        rebuild_tab_display(window, bufs, &tab_id);
    }
}

/// Refresh only the lightweight selection overlay. Dragging used to call
/// `rebuild_tab_display` for every mouse-move event, reparsing and rebuilding
/// all terminal spans even though the underlying screen had not changed.
pub(super) fn refresh_terminal_selection(win: &AppWindow, bufs: &TermBuffers, tab_id: &str) {
    let selection = with_term_buf(bufs, tab_id, |buf| {
        let cols = buf.parser.screen().size().1;
        buf.selection_rects_visible(cols)
    });
    let Some(selection) = selection else {
        return;
    };
    let mut changed = false;
    set_terminal_row(win, tab_id, |row| {
        changed = sync_vec_model_rows(&row.selection, &selection, term_match_eq);
    });
    if changed {
        win.window().request_redraw();
    }
}

/// Resolve the user's saved theme preference to a dark/light bool (mirrors the
/// startup logic): "light"/"dark" win; otherwise ask the OS, defaulting to dark.
pub(super) fn theme_pref_is_dark(store: &ConfigStore) -> bool {
    match store.theme_pref() {
        "light" => false,
        "dark" => true,
        _ => match dark_light::detect() {
            dark_light::Mode::Light => false,
            dark_light::Mode::Dark => true,
            dark_light::Mode::Default => true, // undetectable → dark
        },
    }
}

/// Flip the whole app between light and dark. Setting `Theme.dark` alone only
/// recolours the Slint chrome — each terminal bakes its ANSI/default colours
/// from a per-buffer `is_dark` flag at render time, so we must also update every
/// buffer and re-render it. Both the theme toggle and wallpaper switching route
/// through here (the proc-window mirror stays with the toggle).
pub(super) fn apply_dark_mode(window: &AppWindow, bufs: &TermBuffers, dark: bool) {
    window.set_dark_mode(dark);
    {
        let handles: Vec<_> = bufs.lock().unwrap().values().cloned().collect();
        for h in handles {
            h.lock().unwrap().is_dark = dark;
        }
    }
    rebuild_all_terminal_displays(window, bufs);
}

pub(super) fn apply_output_highlight(
    window: &AppWindow,
    bufs: &TermBuffers,
    enabled: bool,
    preset: &str,
) {
    let mode = OutputHighlightPreset::from_settings(enabled, preset);
    {
        let handles: Vec<_> = bufs.lock().unwrap().values().cloned().collect();
        for handle in handles {
            handle.lock().unwrap().output_highlight = mode;
        }
    }
    let tab_ids: Vec<String> = bufs.lock().unwrap().keys().cloned().collect();
    for tab_id in tab_ids {
        rebuild_tab_display(window, bufs, &tab_id);
    }
}

pub(super) fn apply_custom_output_rules(
    window: &AppWindow,
    bufs: &TermBuffers,
    rules: &[OutputHighlightRule],
) {
    let compiled = compile_output_rules(rules);
    {
        let handles: Vec<_> = bufs.lock().unwrap().values().cloned().collect();
        for handle in handles {
            handle.lock().unwrap().custom_highlight_rules = compiled.clone();
        }
    }
    let tab_ids: Vec<String> = bufs.lock().unwrap().keys().cloned().collect();
    for tab_id in tab_ids {
        rebuild_tab_display(window, bufs, &tab_id);
    }
}

/// Apply a wallpaper id to the window: load the image + derived palette, push the
/// immersive Theme overrides (accent / tint / image) and set `dark` from the
/// image luminance. An empty or undecodable id turns immersive mode off and
/// restores the user's saved light/dark theme.
pub(super) fn apply_wallpaper(
    window: &AppWindow,
    store: &ConfigStore,
    bufs: &TermBuffers,
    id: &str,
    apply_builtin_theme: bool,
) {
    match crate::wallpaper::load(id) {
        Some(wp) => {
            let (ar, ag, ab) = wp.palette.accent;
            let (tr, tg, tb) = wp.palette.tint;
            window.set_wallpaper_img(wp.image);
            window.set_wp_accent(slint::Color::from_rgb_u8(ar, ag, ab));
            window.set_wp_tint(slint::Color::from_rgb_u8(tr, tg, tb));
            // Only the built-ins (designed as a light/dark pair) auto-set the
            // theme. A custom photo keeps the user's light/dark choice so the
            // theme toggle still governs text contrast — a light/white wallpaper
            // reads best in light mode (crisp dark text) rather than being forced
            // dark and greying the text out (#wallpaper).
            if apply_builtin_theme && crate::wallpaper::is_builtin(id) {
                apply_dark_mode(window, bufs, wp.palette.is_dark);
            }
            window.set_wallpaper_active(true);
            window.set_current_wallpaper(id.into());
            let name = if crate::wallpaper::is_builtin(id) {
                String::new()
            } else {
                std::path::Path::new(id)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            };
            window.set_custom_wallpaper_name(name.into());
        }
        None => {
            window.set_wallpaper_active(false);
            window.set_current_wallpaper("".into());
            window.set_custom_wallpaper_name("".into());
            apply_dark_mode(window, bufs, theme_pref_is_dark(store));
        }
    }
    crate::memory_trim::request_idle_trim();
}

/// Resolve which interface drives the top sparkline: the user's selection if it
/// still exists, otherwise the busiest (the list is sorted busiest-first).
/// Returns (name, rx_bps, tx_bps).
pub(super) fn selected_iface(st: &TabStatus) -> (String, u64, u64) {
    if !st.selected_iface.is_empty() {
        if let Some(e) = st.net.iter().find(|e| e.0 == st.selected_iface) {
            return e.clone();
        }
    }
    st.net.first().cloned().unwrap_or_default()
}

/// Recompute the whole sidebar (status dot + CPU/mem/swap + dual network panel)
/// for whichever tab is active.  Welcome tab → local machine; a session tab →
/// that server.  The bottom network graph is always the local machine.
/// Must run on the Slint event loop thread.
/// The copyable IP/host from a `user@host` connection label (#192): the part
/// after the last `@`, trimmed. Falls back to the whole string when there's no
/// `@` (already a bare host/IP).
pub(super) fn conn_ip(host: &str) -> String {
    host.rsplit('@').next().unwrap_or(host).trim().to_string()
}

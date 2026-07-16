//! Cached, off-UI-thread rasterization for visible terminal rows.

use cosmic_text::{
    Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache, Weight, Wrap,
};
use std::borrow::Cow;
use std::collections::{hash_map::DefaultHasher, HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use unicode_width::UnicodeWidthChar;

const MAX_ROW_WIDTH: u32 = 32_768;
const MAX_ROW_HEIGHT: u32 = 256;
const MAX_ROW_BYTES: usize = 32 * 1024 * 1024;
const MAX_GLYPH_CACHE_ENTRIES: usize = 4_096;
const DEFAULT_SHAPE_CACHE_AGES: u64 = 8;
const MAX_SHAPE_CACHE_AGES: u64 = 256;

fn shape_cache_ages() -> u64 {
    static AGES: OnceLock<u64> = OnceLock::new();
    *AGES.get_or_init(|| {
        std::env::var("MEATSHELL_TERMINAL_SHAPE_CACHE_AGES")
            .ok()
            .and_then(|value| value.trim().parse().ok())
            .unwrap_or(DEFAULT_SHAPE_CACHE_AGES)
            .min(MAX_SHAPE_CACHE_AGES)
    })
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct RasterSpan {
    pub text: String,
    pub foreground: [u8; 4],
    pub background: [u8; 4],
    pub bold: bool,
    pub col: u32,
    pub cells: u32,
    pub cjk: bool,
}

#[derive(Clone, Debug)]
pub struct RasterConfig {
    pub columns: u32,
    /// Physical pixels per terminal cell.
    pub cell_width: f32,
    pub cell_height: f32,
    pub font_size: f32,
    pub terminal_family: String,
    pub cjk_family: String,
    pub force_bold: bool,
}

impl RasterConfig {
    fn hash_into(&self, hasher: &mut impl Hasher) {
        self.columns.hash(hasher);
        self.cell_width.to_bits().hash(hasher);
        self.cell_height.to_bits().hash(hasher);
        self.font_size.to_bits().hash(hasher);
        self.terminal_family.hash(hasher);
        self.cjk_family.hash(hasher);
        self.force_bold.hash(hasher);
    }

    fn pixel_size(&self) -> Result<(u32, u32), String> {
        let width = (self.columns as f32 * self.cell_width).round() as u32;
        let height = self.cell_height.round() as u32;
        if width == 0 || height == 0 || width > MAX_ROW_WIDTH || height > MAX_ROW_HEIGHT {
            return Err(format!("invalid terminal row image size {width}x{height}"));
        }
        let bytes = width as usize * height as usize * 4;
        if bytes > MAX_ROW_BYTES {
            return Err(format!("terminal row image is too large: {bytes} bytes"));
        }
        Ok((width, height))
    }
}

#[derive(Debug)]
pub struct RasterJob {
    tab_id: String,
    generation: u64,
    epoch: u64,
    reset_cache: bool,
    config: RasterConfig,
    rows: Vec<Vec<RasterSpan>>,
    row_signatures: Vec<u64>,
    frame_key: u64,
    logical_cell_width: f32,
    logical_cell_height: f32,
    cursor_row: i32,
    cursor_col: i32,
}

impl RasterJob {
    pub fn new(tab_id: String, config: RasterConfig, rows: Vec<Vec<RasterSpan>>) -> Self {
        let row_signatures: Vec<u64> = rows
            .iter()
            .map(|row| {
                let mut hasher = DefaultHasher::new();
                config.hash_into(&mut hasher);
                row.hash(&mut hasher);
                hasher.finish()
            })
            .collect();
        let logical_cell_width = config.cell_width;
        let logical_cell_height = config.cell_height;
        let mut job = Self {
            tab_id,
            generation: 0,
            epoch: 0,
            reset_cache: false,
            config,
            rows,
            row_signatures,
            frame_key: 0,
            logical_cell_width,
            logical_cell_height,
            cursor_row: 0,
            cursor_col: 0,
        };
        job.refresh_frame_key();
        job
    }

    /// Attach the logical grid geometry and cursor represented by this image
    /// frame. Keeping these values in the result prevents a newer native cursor
    /// from being drawn over an older asynchronously-rasterized text frame.
    pub fn with_frame_state(
        mut self,
        logical_cell_width: f32,
        logical_cell_height: f32,
        cursor_row: i32,
        cursor_col: i32,
    ) -> Self {
        self.logical_cell_width = logical_cell_width;
        self.logical_cell_height = logical_cell_height;
        self.cursor_row = cursor_row;
        self.cursor_col = cursor_col;
        self.refresh_frame_key();
        self
    }

    fn refresh_frame_key(&mut self) {
        let mut hasher = DefaultHasher::new();
        self.config.hash_into(&mut hasher);
        self.row_signatures.hash(&mut hasher);
        self.logical_cell_width.to_bits().hash(&mut hasher);
        self.logical_cell_height.to_bits().hash(&mut hasher);
        self.cursor_row.hash(&mut hasher);
        self.cursor_col.hash(&mut hasher);
        self.frame_key = hasher.finish();
    }
}

#[derive(Debug)]
pub struct RasterRowPixels {
    pub row: usize,
    pub width: u32,
    pub height: u32,
    /// Premultiplied RGBA8 pixels.
    pub pixels: Vec<u8>,
}

#[derive(Debug)]
pub struct RasterResult {
    pub tab_id: String,
    pub generation: u64,
    epoch: u64,
    pub row_count: usize,
    pub columns: u32,
    pub logical_cell_width: f32,
    pub logical_cell_height: f32,
    pub cursor_row: i32,
    pub cursor_col: i32,
    pub has_content: bool,
    pub updates: Vec<RasterRowPixels>,
}

impl RasterResult {
    /// Merge a newer delta into a result that has not reached the UI yet.
    pub fn merge_from(&mut self, mut newer: Self) {
        self.generation = newer.generation;
        self.epoch = newer.epoch;
        self.row_count = newer.row_count;
        self.columns = newer.columns;
        self.logical_cell_width = newer.logical_cell_width;
        self.logical_cell_height = newer.logical_cell_height;
        self.cursor_row = newer.cursor_row;
        self.cursor_col = newer.cursor_col;
        self.has_content = newer.has_content;
        self.updates.retain(|update| update.row < newer.row_count);
        for update in newer.updates.drain(..) {
            if let Some(existing) = self
                .updates
                .iter_mut()
                .find(|existing| existing.row == update.row)
            {
                *existing = update;
            } else {
                self.updates.push(update);
            }
        }
    }
}

pub type RasterCompletion = Result<RasterResult, String>;
type Completion = Box<dyn FnOnce(RasterCompletion) + Send + 'static>;

struct Envelope {
    job: RasterJob,
    completion: Completion,
}

#[derive(Default)]
struct QueueState {
    pending: HashMap<String, Envelope>,
    order: VecDeque<String>,
    last_frame_keys: HashMap<String, u64>,
    generations: HashMap<String, u64>,
    epochs: HashMap<String, u64>,
    reset_tabs: HashSet<String>,
}

#[derive(Default)]
struct SharedQueue {
    state: Mutex<QueueState>,
    ready: Condvar,
}

impl SharedQueue {
    fn take(&self) -> Envelope {
        let mut state = self.state.lock().unwrap();
        loop {
            while let Some(tab_id) = state.order.pop_front() {
                if let Some(envelope) = state.pending.remove(&tab_id) {
                    return envelope;
                }
            }
            state = self.ready.wait(state).unwrap();
        }
    }
}

#[derive(Clone)]
pub struct TerminalRasterizer {
    queue: Arc<SharedQueue>,
}

impl TerminalRasterizer {
    pub fn new() -> Self {
        let queue = Arc::new(SharedQueue::default());
        let worker_queue = queue.clone();
        std::thread::Builder::new()
            .name("terminal-row-raster".to_string())
            .spawn(move || worker_loop(worker_queue))
            .expect("failed to spawn terminal row rasterizer");
        Self { queue }
    }

    /// Queue one frame. A tab can have at most one waiting frame; a newer frame
    /// replaces it while the worker finishes the frame already in progress.
    pub fn submit(
        &self,
        mut job: RasterJob,
        completion: impl FnOnce(RasterCompletion) + Send + 'static,
    ) -> bool {
        let mut state = self.queue.state.lock().unwrap();
        if state.last_frame_keys.get(&job.tab_id) == Some(&job.frame_key) {
            return false;
        }
        state
            .last_frame_keys
            .insert(job.tab_id.clone(), job.frame_key);
        let generation = state.generations.entry(job.tab_id.clone()).or_default();
        *generation = generation.wrapping_add(1);
        job.generation = *generation;
        job.epoch = state.epochs.get(&job.tab_id).copied().unwrap_or_default();
        job.reset_cache = state.reset_tabs.remove(&job.tab_id);

        if !state.pending.contains_key(&job.tab_id) {
            state.order.push_back(job.tab_id.clone());
        }
        state.pending.insert(
            job.tab_id.clone(),
            Envelope {
                job,
                completion: Box::new(completion),
            },
        );
        drop(state);
        self.queue.ready.notify_one();
        true
    }

    pub fn forget(&self, tab_id: &str) {
        let mut state = self.queue.state.lock().unwrap();
        state.pending.remove(tab_id);
        state.last_frame_keys.remove(tab_id);
        state.generations.remove(tab_id);
        let epoch = state.epochs.entry(tab_id.to_string()).or_default();
        *epoch = epoch.wrapping_add(1);
        state.reset_tabs.insert(tab_id.to_string());
    }

    /// True when `generation` is still the newest frame submitted for `tab_id`.
    /// Older deltas may still be applied to the hidden image cache, but must not
    /// make that cache visible over a newer terminal span frame.
    pub fn is_latest_generation(&self, tab_id: &str, generation: u64) -> bool {
        self.queue
            .state
            .lock()
            .unwrap()
            .generations
            .get(tab_id)
            .is_some_and(|latest| *latest == generation)
    }
}

fn worker_loop(queue: Arc<SharedQueue>) {
    let mut worker: Option<WorkerState> = None;
    loop {
        let envelope = queue.take();
        let result = catch_unwind(AssertUnwindSafe(|| {
            worker
                .get_or_insert_with(WorkerState::new)
                .render(envelope.job)
        }))
        .unwrap_or_else(|panic| {
            worker = None;
            let message = panic
                .downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "terminal row rasterizer panicked".to_string());
            Err(message)
        });
        let result_epoch = result.as_ref().ok().map(|result| result.epoch);
        let result_tab = result.as_ref().ok().map(|result| result.tab_id.as_str());
        let still_current = match (result_tab, result_epoch) {
            (Some(tab_id), Some(epoch)) => {
                queue
                    .state
                    .lock()
                    .unwrap()
                    .epochs
                    .get(tab_id)
                    .copied()
                    .unwrap_or_default()
                    == epoch
            }
            _ => true,
        };
        if still_current {
            // A UI completion must never be able to kill the sole raster worker.
            // Keep processing future frames even if event-loop teardown or a
            // poisoned downstream lock makes one callback panic.
            if catch_unwind(AssertUnwindSafe(|| (envelope.completion)(result))).is_err() {
                tracing::warn!("terminal row raster completion panicked");
            }
        }
    }
}

struct WorkerState {
    font_system: FontSystem,
    glyph_cache: SwashCache,
    row_cache: HashMap<String, Vec<u64>>,
}

impl WorkerState {
    fn new() -> Self {
        let fonts = [
            fontdb::Source::Binary(Arc::new(
                include_bytes!("../ui/fonts/MeatshellMono-Regular.ttf").to_vec(),
            )),
            fontdb::Source::Binary(Arc::new(
                include_bytes!("../ui/fonts/MeatshellMono-Bold.ttf").to_vec(),
            )),
        ];
        Self {
            font_system: FontSystem::new_with_fonts(fonts),
            glyph_cache: SwashCache::new(),
            row_cache: HashMap::new(),
        }
    }

    fn render(&mut self, job: RasterJob) -> RasterCompletion {
        let (width, height) = job.config.pixel_size()?;
        let has_content = job.rows.iter().flatten().any(|span| {
            span.background[3] != 0
                || (span.foreground[3] != 0 && span.text.chars().any(|ch| !ch.is_whitespace()))
        });
        if job.reset_cache {
            self.row_cache.remove(&job.tab_id);
        }
        let cached = self.row_cache.entry(job.tab_id.clone()).or_default();
        let mut updates = Vec::new();

        for (row_index, row) in job.rows.iter().enumerate() {
            if cached.get(row_index) == job.row_signatures.get(row_index) {
                continue;
            }
            updates.push(RasterRowPixels {
                row: row_index,
                width,
                height,
                pixels: render_row(
                    &mut self.font_system,
                    &mut self.glyph_cache,
                    &job.config,
                    row,
                    width,
                    height,
                )?,
            });
        }

        if self.glyph_cache.image_cache.len() > MAX_GLYPH_CACHE_ENTRIES {
            self.glyph_cache.image_cache.clear();
        }
        self.font_system.shape_run_cache.trim(shape_cache_ages());

        *cached = job.row_signatures;
        Ok(RasterResult {
            tab_id: job.tab_id,
            generation: job.generation,
            epoch: job.epoch,
            row_count: job.rows.len(),
            columns: job.config.columns,
            logical_cell_width: job.logical_cell_width,
            logical_cell_height: job.logical_cell_height,
            cursor_row: job.cursor_row,
            cursor_col: job.cursor_col,
            has_content,
            updates,
        })
    }
}

struct TextPiece<'a> {
    text: Cow<'a, str>,
    span: Option<&'a RasterSpan>,
    start_col: f32,
    end_col: f32,
}

fn render_row(
    font_system: &mut FontSystem,
    glyph_cache: &mut SwashCache,
    config: &RasterConfig,
    row: &[RasterSpan],
    width: u32,
    height: u32,
) -> Result<Vec<u8>, String> {
    let mut pixels = vec![0u8; width as usize * height as usize * 4];
    for span in row {
        fill_background(&mut pixels, width, height, config, span);
    }

    let mut pieces = Vec::with_capacity(row.len() * 2 + 1);
    let mut cell = 0u32;
    for span in row {
        if span.col > cell {
            pieces.push(TextPiece {
                text: Cow::Owned(" ".repeat((span.col - cell) as usize)),
                span: None,
                start_col: cell as f32,
                end_col: span.col as f32,
            });
        }
        let span_end = span.col.saturating_add(span.cells);
        pieces.push(TextPiece {
            text: Cow::Borrowed(&span.text),
            span: Some(span),
            start_col: span.col as f32,
            end_col: span_end as f32,
        });
        cell = span_end;
    }
    if cell < config.columns {
        pieces.push(TextPiece {
            text: Cow::Owned(" ".repeat((config.columns - cell) as usize)),
            span: None,
            start_col: cell as f32,
            end_col: config.columns as f32,
        });
    }
    let byte_cells = byte_cell_positions(&pieces);

    let terminal_family = if config.terminal_family.trim().is_empty() {
        "Meatshell Mono"
    } else {
        config.terminal_family.as_str()
    };
    let default_attrs =
        Attrs::new()
            .family(Family::Name(terminal_family))
            .weight(if config.force_bold {
                Weight::BOLD
            } else {
                Weight::NORMAL
            });
    let rich_text: Vec<_> = pieces
        .iter()
        .map(|piece| {
            let attrs = if let Some(span) = piece.span {
                let family = if span.cjk && !config.cjk_family.trim().is_empty() {
                    config.cjk_family.as_str()
                } else {
                    terminal_family
                };
                Attrs::new()
                    .family(Family::Name(family))
                    .weight(if config.force_bold || span.bold {
                        Weight::BOLD
                    } else {
                        Weight::NORMAL
                    })
                    .color(Color::rgba(
                        span.foreground[0],
                        span.foreground[1],
                        span.foreground[2],
                        span.foreground[3],
                    ))
            } else {
                default_attrs.clone()
            };
            (piece.text.as_ref(), attrs)
        })
        .collect();

    let metrics = Metrics::new(config.font_size, config.cell_height);
    let mut buffer = Buffer::new(font_system, metrics);
    buffer.set_size(font_system, Some(width as f32), Some(height as f32));
    buffer.set_wrap(font_system, Wrap::None);
    buffer.set_rich_text(
        font_system,
        rich_text,
        &default_attrs,
        Shaping::Advanced,
        None,
    );
    buffer.shape_until_scroll(font_system, false);

    // Cosmic Text chooses fonts and shapes clusters, but its natural advances
    // are not guaranteed to match Slint's measured terminal cells. Anchor each
    // shaped cluster to the vt100 cell map so cursor, selection and text cannot
    // drift apart horizontally as a line gets longer.
    for run in buffer.layout_runs() {
        let mut cluster_bounds: HashMap<(usize, usize), (f32, f32)> = HashMap::new();
        for glyph in run.glyphs {
            cluster_bounds
                .entry((glyph.start, glyph.end))
                .and_modify(|bounds| {
                    bounds.0 = bounds.0.min(glyph.x);
                    bounds.1 = bounds.1.max(glyph.x + glyph.w);
                })
                .or_insert((glyph.x, glyph.x + glyph.w));
        }

        for glyph in run.glyphs {
            let start = byte_cells
                .get(glyph.start)
                .copied()
                .unwrap_or(config.columns as f32);
            let end = byte_cells.get(glyph.end).copied().unwrap_or(start);
            let (natural_start, natural_end) = cluster_bounds
                .get(&(glyph.start, glyph.end))
                .copied()
                .unwrap_or((glyph.x, glyph.x + glyph.w));
            let target_width = (end - start).max(0.0) * config.cell_width;
            let natural_width = (natural_end - natural_start).max(0.0);
            let centered_x = start * config.cell_width + (target_width - natural_width) * 0.5;
            let physical = glyph.physical((centered_x - natural_start, 0.0), 1.0);
            let color = glyph.color_opt.unwrap_or_else(|| Color::rgb(255, 255, 255));
            glyph_cache.with_pixels(
                font_system,
                physical.cache_key,
                color,
                |glyph_x, glyph_y, color| {
                    let x = physical.x + glyph_x;
                    let y = run.line_y as i32 + physical.y + glyph_y;
                    if x < 0 || y < 0 || x >= width as i32 || y >= height as i32 {
                        return;
                    }
                    let index = (y as usize * width as usize + x as usize) * 4;
                    blend_premultiplied(&mut pixels[index..index + 4], color.as_rgba());
                },
            );
        }
    }
    Ok(pixels)
}

/// Map every UTF-8 byte boundary in the shaped row to an authoritative vt100
/// column. A piece's declared cell count wins over font or Unicode-width guesses;
/// Unicode widths are used only to distribute characters within that piece.
fn byte_cell_positions(pieces: &[TextPiece<'_>]) -> Vec<f32> {
    let byte_len = pieces.iter().map(|piece| piece.text.len()).sum::<usize>();
    let mut positions = vec![0.0; byte_len.saturating_add(1)];
    let mut byte_base = 0usize;

    for piece in pieces {
        let units = piece
            .text
            .chars()
            .map(|ch| ch.width().unwrap_or(0) as f32)
            .sum::<f32>();
        let cell_span = piece.end_col - piece.start_col;
        let mut used = 0.0f32;

        for (offset, ch) in piece.text.char_indices() {
            let cell = if units > 0.0 {
                piece.start_col + cell_span * used / units
            } else {
                piece.start_col
            };
            let start = byte_base + offset;
            let end = start + ch.len_utf8();
            positions[start..end].fill(cell);
            used += ch.width().unwrap_or(0) as f32;
        }
        byte_base += piece.text.len();
        positions[byte_base] = piece.end_col;
    }

    positions
}

fn fill_background(
    pixels: &mut [u8],
    width: u32,
    height: u32,
    config: &RasterConfig,
    span: &RasterSpan,
) {
    if span.background[3] == 0 {
        return;
    }
    let x0 = (span.col as f32 * config.cell_width).round() as u32;
    let x1 = (span.col.saturating_add(span.cells) as f32 * config.cell_width).round() as u32;
    let x0 = x0.min(width);
    let x1 = x1.min(width);
    let color = premultiply(span.background);
    for y in 0..height {
        for x in x0..x1 {
            let index = (y as usize * width as usize + x as usize) * 4;
            pixels[index..index + 4].copy_from_slice(&color);
        }
    }
}

fn premultiply([r, g, b, a]: [u8; 4]) -> [u8; 4] {
    let mul = |channel: u8| ((channel as u16 * a as u16 + 127) / 255) as u8;
    [mul(r), mul(g), mul(b), a]
}

fn blend_premultiplied(destination: &mut [u8], [r, g, b, a]: [u8; 4]) {
    let inverse = 255u16 - a as u16;
    let blend = |source: u8, target: u8| {
        ((source as u16 * a as u16 + target as u16 * inverse + 127) / 255) as u8
    };
    destination[0] = blend(r, destination[0]);
    destination[1] = blend(g, destination[1]);
    destination[2] = blend(b, destination[2]);
    destination[3] = (a as u16 + (destination[3] as u16 * inverse + 127) / 255) as u8;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> RasterConfig {
        RasterConfig {
            columns: 8,
            cell_width: 8.0,
            cell_height: 18.0,
            font_size: 13.0,
            terminal_family: "Meatshell Mono".to_string(),
            cjk_family: String::new(),
            force_bold: false,
        }
    }

    fn row(text: &str, foreground: [u8; 4]) -> Vec<RasterSpan> {
        vec![RasterSpan {
            text: format!("{text:<8}"),
            foreground,
            background: [0, 0, 0, 0],
            bold: false,
            col: 0,
            cells: 8,
            cjk: false,
        }]
    }

    #[test]
    fn premultiplied_blending_preserves_alpha_and_color() {
        let mut pixel = [0, 0, 0, 0];
        blend_premultiplied(&mut pixel, [200, 100, 50, 128]);
        assert_eq!(pixel, [100, 50, 25, 128]);
        blend_premultiplied(&mut pixel, [0, 0, 255, 255]);
        assert_eq!(pixel, [0, 0, 255, 255]);
    }

    #[test]
    fn row_cache_returns_only_changed_rows() {
        let mut worker = WorkerState::new();
        let first = worker
            .render(RasterJob::new(
                "tab".to_string(),
                config(),
                vec![
                    row("one", [255, 255, 255, 255]),
                    row("two", [255, 0, 0, 255]),
                ],
            ))
            .unwrap();
        assert_eq!(first.updates.len(), 2);
        assert!(first
            .updates
            .iter()
            .all(|update| update.pixels.iter().any(|v| *v != 0)));

        let unchanged = worker
            .render(RasterJob::new(
                "tab".to_string(),
                config(),
                vec![
                    row("one", [255, 255, 255, 255]),
                    row("two", [255, 0, 0, 255]),
                ],
            ))
            .unwrap();
        assert!(unchanged.updates.is_empty());

        let changed = worker
            .render(RasterJob::new(
                "tab".to_string(),
                config(),
                vec![
                    row("one", [255, 255, 255, 255]),
                    row("new", [255, 0, 0, 255]),
                ],
            ))
            .unwrap();
        assert_eq!(changed.updates.len(), 1);
        assert_eq!(changed.updates[0].row, 1);
    }

    #[test]
    fn erasing_a_row_emits_a_transparent_replacement() {
        let mut worker = WorkerState::new();
        worker
            .render(RasterJob::new(
                "tab".to_string(),
                config(),
                vec![row("docke", [255, 255, 255, 255])],
            ))
            .unwrap();

        let erased = worker
            .render(RasterJob::new(
                "tab".to_string(),
                config(),
                vec![Vec::new()],
            ))
            .unwrap();
        assert_eq!(erased.updates.len(), 1);
        assert_eq!(erased.updates[0].row, 0);
        assert!(erased.updates[0].pixels.iter().all(|value| *value == 0));
    }

    #[test]
    fn transparent_placeholder_frame_is_not_content() {
        let mut worker = WorkerState::new();
        let result = worker
            .render(RasterJob::new(
                "empty".to_string(),
                config(),
                vec![Vec::new(), Vec::new()],
            ))
            .unwrap();
        assert!(!result.has_content);
        assert_eq!(result.updates.len(), 2);
    }

    #[test]
    fn pending_queue_keeps_only_the_latest_frame_per_tab() {
        let rasterizer = TerminalRasterizer {
            queue: Arc::new(SharedQueue::default()),
        };
        assert!(rasterizer.submit(
            RasterJob::new(
                "tab".to_string(),
                config(),
                vec![row("old", [255, 255, 255, 255])],
            ),
            |_| {},
        ));
        assert!(rasterizer.submit(
            RasterJob::new(
                "tab".to_string(),
                config(),
                vec![row("new", [255, 255, 255, 255])],
            ),
            |_| {},
        ));

        let state = rasterizer.queue.state.lock().unwrap();
        assert_eq!(state.pending.len(), 1);
        assert_eq!(state.order.len(), 1);
        let pending = state.pending.get("tab").unwrap();
        assert!(pending.job.rows[0][0].text.starts_with("new"));
    }

    #[test]
    fn latest_generation_advances_when_a_waiting_frame_is_replaced() {
        let rasterizer = TerminalRasterizer {
            queue: Arc::new(SharedQueue::default()),
        };
        assert!(rasterizer.submit(
            RasterJob::new(
                "tab".to_string(),
                config(),
                vec![row("old", [255, 255, 255, 255])],
            ),
            |_| {},
        ));
        assert!(rasterizer.is_latest_generation("tab", 1));

        assert!(rasterizer.submit(
            RasterJob::new(
                "tab".to_string(),
                config(),
                vec![row("new", [255, 255, 255, 255])],
            ),
            |_| {},
        ));
        assert!(!rasterizer.is_latest_generation("tab", 1));
        assert!(rasterizer.is_latest_generation("tab", 2));
    }

    #[test]
    fn completed_deltas_merge_without_losing_older_dirty_rows() {
        let mut worker = WorkerState::new();
        let mut first = worker
            .render(RasterJob::new(
                "tab".to_string(),
                config(),
                vec![
                    row("one", [255, 255, 255, 255]),
                    row("two", [255, 0, 0, 255]),
                ],
            ))
            .unwrap();
        let second = worker
            .render(RasterJob::new(
                "tab".to_string(),
                config(),
                vec![
                    row("one", [255, 255, 255, 255]),
                    row("new", [255, 0, 0, 255]),
                ],
            ))
            .unwrap();

        first.merge_from(second);
        first.updates.sort_by_key(|update| update.row);
        assert_eq!(
            first
                .updates
                .iter()
                .map(|update| update.row)
                .collect::<Vec<_>>(),
            vec![0, 1]
        );
    }

    #[test]
    fn completed_delta_merge_prefers_newer_rows_and_drops_truncated_rows() {
        let update = |row, value| RasterRowPixels {
            row,
            width: 1,
            height: 1,
            pixels: vec![value; 4],
        };
        let mut pending = RasterResult {
            tab_id: "tab".to_string(),
            generation: 1,
            epoch: 0,
            row_count: 3,
            columns: 8,
            logical_cell_width: 8.0,
            logical_cell_height: 18.0,
            cursor_row: 1,
            cursor_col: 2,
            has_content: true,
            updates: vec![update(0, 10), update(1, 20), update(2, 30)],
        };
        let newer = RasterResult {
            tab_id: "tab".to_string(),
            generation: 2,
            epoch: 0,
            row_count: 2,
            columns: 6,
            logical_cell_width: 9.0,
            logical_cell_height: 19.0,
            cursor_row: 0,
            cursor_col: 4,
            has_content: false,
            updates: vec![update(1, 99)],
        };

        pending.merge_from(newer);
        pending.updates.sort_by_key(|update| update.row);

        assert_eq!(pending.generation, 2);
        assert_eq!(pending.row_count, 2);
        assert_eq!(pending.columns, 6);
        assert_eq!(pending.logical_cell_width, 9.0);
        assert_eq!((pending.cursor_row, pending.cursor_col), (0, 4));
        assert!(!pending.has_content);
        assert_eq!(pending.updates.len(), 2);
        assert_eq!(pending.updates[0].row, 0);
        assert_eq!(pending.updates[0].pixels, vec![10; 4]);
        assert_eq!(pending.updates[1].row, 1);
        assert_eq!(pending.updates[1].pixels, vec![99; 4]);
    }

    #[test]
    fn byte_cell_map_uses_authoritative_piece_geometry() {
        let pieces = vec![
            TextPiece {
                text: Cow::Borrowed("ab"),
                span: None,
                start_col: 3.0,
                end_col: 5.0,
            },
            TextPiece {
                text: Cow::Borrowed("中"),
                span: None,
                start_col: 5.0,
                end_col: 7.0,
            },
        ];
        let cells = byte_cell_positions(&pieces);
        assert_eq!(cells[0], 3.0);
        assert_eq!(cells[1], 4.0);
        assert_eq!(cells[2], 5.0);
        assert_eq!(cells[5], 7.0);
    }
}

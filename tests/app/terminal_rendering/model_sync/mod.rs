use super::*;

fn span(text: &str) -> TermSpan {
    TermSpan {
        text: text.into(),
        fg: slint::Color::from_rgb_u8(220, 220, 220),
        bg: slint::Color::from_argb_u8(0, 0, 0, 0),
        bold: false,
        row: 1,
        col: 2,
        cells: 3,
        cjk: false,
        emoji: false,
        emoji_image: slint::Image::default(),
    }
}

fn model_matches(model: &VecModel<TermSpan>, expected: &[TermSpan]) -> bool {
    model.row_count() == expected.len()
        && expected.iter().enumerate().all(|(index, item)| {
            model
                .row_data(index)
                .map(|actual| term_span_eq(&actual, item))
                .unwrap_or(false)
        })
}

#[test]
fn terminal_span_sync_reuses_model_and_diffs_rows_and_tail() {
    let initial = vec![span("one"), span("two")];
    let concrete = Rc::new(VecModel::from(initial.clone()));
    let model = ModelRc::from(concrete.clone());
    let downcast = model.as_any().downcast_ref::<VecModel<TermSpan>>().unwrap();
    assert!(std::ptr::eq(downcast, concrete.as_ref()));

    assert!(!sync_term_span_rows(&model, &initial));
    assert!(model_matches(&concrete, &initial));

    let mut changed = initial.clone();
    changed[1].text = "updated".into();
    assert!(sync_term_span_rows(&model, &changed));
    assert!(model_matches(&concrete, &changed));

    let mut grown = changed.clone();
    grown.push(span("three"));
    assert!(sync_term_span_rows(&model, &grown));
    assert!(model_matches(&concrete, &grown));

    let shrunk = vec![grown[0].clone()];
    assert!(sync_term_span_rows(&model, &shrunk));
    assert!(model_matches(&concrete, &shrunk));
    assert!(std::ptr::eq(
        model.as_any().downcast_ref::<VecModel<TermSpan>>().unwrap(),
        concrete.as_ref()
    ));
}

#[test]
fn terminal_span_sync_aligns_run_splits_by_grid_position() {
    let mut row_zero = span("row zero");
    row_zero.row = 0;
    row_zero.col = 0;
    let mut row_one = span("row one");
    row_one.row = 1;
    row_one.col = 0;
    let mut row_two = span("row two");
    row_two.row = 2;
    row_two.col = 0;

    let concrete = Rc::new(VecModel::from(vec![
        row_zero.clone(),
        row_one.clone(),
        row_two.clone(),
    ]));
    let model = ModelRc::from(concrete.clone());

    let mut split_left = row_zero.clone();
    split_left.text = "row ".into();
    split_left.cells = 4;
    let mut split_right = row_zero;
    split_right.text = "zero".into();
    split_right.col = 4;
    split_right.cells = 4;
    let expected = vec![split_left, split_right, row_one, row_two];

    assert!(sync_term_span_rows(&model, &expected));
    assert!(model_matches(&concrete, &expected));
    assert!(!sync_term_span_rows(&model, &expected));
}

#[test]
fn terminal_span_comparison_covers_every_rendered_field() {
    let original = span("same");

    let mut variants = Vec::new();
    let mut changed = original.clone();
    changed.text = "different".into();
    variants.push(changed);
    let mut changed = original.clone();
    changed.fg = slint::Color::from_rgb_u8(1, 2, 3);
    variants.push(changed);
    let mut changed = original.clone();
    changed.bg = slint::Color::from_rgb_u8(4, 5, 6);
    variants.push(changed);
    let mut changed = original.clone();
    changed.bold = true;
    variants.push(changed);
    let mut changed = original.clone();
    changed.row += 1;
    variants.push(changed);
    let mut changed = original.clone();
    changed.col += 1;
    variants.push(changed);
    let mut changed = original.clone();
    changed.cells += 1;
    variants.push(changed);
    let mut changed = original.clone();
    changed.cjk = true;
    variants.push(changed);
    let mut changed = original.clone();
    changed.emoji = true;
    variants.push(changed);

    assert!(variants
        .iter()
        .all(|changed| !term_span_eq(&original, changed)));

    // Raster image identity matters for emoji spans, but is intentionally
    // ignored for plain text where the image field is not rendered.
    let mut emoji = original.clone();
    emoji.emoji = true;
    let mut emoji_with_image = emoji.clone();
    emoji_with_image.emoji_image = slint::Image::from_rgba8_premultiplied(
        slint::SharedPixelBuffer::clone_from_slice(&[255, 0, 0, 255], 1, 1),
    );
    assert!(!term_span_eq(&emoji, &emoji_with_image));
    let mut plain_with_image = original.clone();
    plain_with_image.emoji_image = emoji_with_image.emoji_image;
    assert!(term_span_eq(&original, &plain_with_image));
}

#[test]
fn terminal_match_comparison_covers_grid_rectangle() {
    let original = TermMatch {
        row: 1,
        col: 2,
        len: 3,
    };
    let mut changed = original.clone();
    changed.row += 1;
    assert!(!term_match_eq(&original, &changed));
    changed = original.clone();
    changed.col += 1;
    assert!(!term_match_eq(&original, &changed));
    changed = original.clone();
    changed.len += 1;
    assert!(!term_match_eq(&original, &changed));
}

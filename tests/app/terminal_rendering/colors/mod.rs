use super::*;

#[test]
fn inverse_default_colours_paint_a_visible_background() {
    let (fg, bg) = vt_span_colors(
        vt100::Color::Default,
        vt100::Color::Default,
        false,
        true,
        true,
    );
    assert_eq!(fg.as_argb_encoded(), 0xff0e0f13);
    assert_eq!(bg.as_argb_encoded(), 0xffd4d4d4);

    let mut parser = vt100::Parser::new(3, 30, 0);
    parser.process(b"abc \x1b[7m20260705\x1b[27m end");
    let (_plain, runs, _wrapped) = build_row(parser.screen(), 0, 30);
    let hit = runs
        .iter()
        .find(|span| span.text.contains("20260705"))
        .expect("reverse-video search hit should be a separate span");
    assert!(hit.inverse);
    assert!(matches!(hit.fg, vt100::Color::Default));
    assert!(matches!(hit.bg, vt100::Color::Default));
}

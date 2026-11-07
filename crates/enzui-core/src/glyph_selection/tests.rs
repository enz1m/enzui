use super::*;

/// Two lines of 10 px glyphs, 12 px tall, 20 px apart.
fn page() -> GlyphText {
    let mut glyphs = Vec::new();
    for (line, words) in ["The quick fox", "jumps over"].iter().enumerate() {
        for (i, ch) in words.chars().enumerate() {
            glyphs.push(Glyph::new(
                ch,
                [i as f32 * 10.0, line as f32 * 20.0, 10.0, 12.0],
                line as u32,
            ));
        }
    }
    GlyphText::new(glyphs)
}

#[test]
fn a_selection_can_start_and_end_mid_line() {
    let t = page();
    // From after "The " to after "jum".
    let range = t.caret_at(41.0, 5.0)..t.caret_at(26.0, 25.0);
    assert_eq!(t.text(range.clone()), "quick fox\njum");
    let rects = t.range_rects(range);
    assert_eq!(rects.len(), 2, "one rectangle per line");
    assert_eq!(
        rects[0],
        [40.0, 0.0, 90.0, 12.0],
        "from 'q' to the line end"
    );
    assert_eq!(
        rects[1],
        [0.0, 20.0, 30.0, 12.0],
        "from the line start to 'm'"
    );
}

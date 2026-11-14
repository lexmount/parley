// Copyright 2026 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use alloc::vec::Vec;

use super::utils::fonts::{FONT_FAMILY_LIST, create_font_context};
use crate::{
    Alignment, AlignmentOptions, BaseDirection, FontFamily, GlyphRun, IndentOptions, LayoutContext,
    PositionedLayoutItem, TextStyle, VerticalAlign,
};

fn glyph_key(glyph: crate::Glyph) -> (u32, u32, u32, u32) {
    (
        glyph.id,
        glyph.x.to_bits(),
        glyph.y.to_bits(),
        glyph.advance.to_bits(),
    )
}

fn segment_slice<'a>(glyphs: &GlyphRun<'a, ()>) -> parley_engine::ShapedSlice<'a> {
    let full = glyphs.run().full_slice();
    let range = glyphs.text_range();
    let first = full
        .atom_at_text_byte(range.start.try_into().unwrap())
        .unwrap();
    let last = full
        .atom_at_text_byte((range.end - 1).try_into().unwrap())
        .unwrap();
    full.narrow(first.shaped_clusters_range().start..last.shaped_clusters_range().end)
}

fn check_views(glyphs: &GlyphRun<'_, ()>) -> usize {
    let complete = glyphs
        .positioned_glyphs()
        .map(glyph_key)
        .collect::<Vec<_>>();
    let full = glyphs.slice_text(glyphs.text_range()).unwrap();
    assert_eq!(
        full.positioned_glyphs().map(glyph_key).collect::<Vec<_>>(),
        complete,
        "a complete view must be bit-identical"
    );
    assert_eq!(full.advance().to_bits(), glyphs.advance().to_bits());
    assert_eq!(full.style_index(), glyphs.style_index());
    assert!(core::ptr::eq(full.run().font(), glyphs.run().font()));
    assert!(core::ptr::eq(full.style(), glyphs.style()));

    let slice = segment_slice(glyphs);
    let mut bounds = Vec::from([glyphs.text_range().start]);
    for atom in slice.atoms_start() {
        bounds.push(slice.text_byte_range(atom.char_range()).end);
    }
    let mut checked = 0;
    for start in 0..bounds.len() - 1 {
        for end in start + 1..bounds.len() {
            let range = bounds[start]..bounds[end];
            let view = glyphs.slice_text(range.clone()).unwrap();
            assert_eq!(view.text_range(), range);
            let mapped = view.glyphs_with_text_ranges().collect::<Vec<_>>();
            assert_eq!(
                mapped
                    .iter()
                    .map(|(glyph, _)| glyph_key(*glyph))
                    .collect::<Vec<_>>(),
                view.glyphs().map(glyph_key).collect::<Vec<_>>(),
                "source mapping must preserve the complete glyph stream"
            );
            assert!(
                mapped
                    .iter()
                    .all(|(_, source)| { range.start <= source.start && source.end <= range.end })
            );
            let expected = glyphs
                .glyphs_with_text_ranges()
                .filter(|(_, source)| range.start <= source.start && source.end <= range.end)
                .map(|(glyph, source)| (glyph_key(glyph), source))
                .collect::<Vec<_>>();
            assert_eq!(
                mapped
                    .into_iter()
                    .map(|(glyph, source)| (glyph_key(glyph), source))
                    .collect::<Vec<_>>(),
                expected,
                "views must retain the original atom/source associations"
            );
            assert!(core::ptr::eq(view.run().font(), glyphs.run().font()));
            assert!(core::ptr::eq(view.style(), glyphs.style()));
            assert_eq!(view.baseline().to_bits(), glyphs.baseline().to_bits());

            let selected = view.positioned_glyphs().map(glyph_key).collect::<Vec<_>>();
            assert!(!selected.is_empty());
            assert!(
                complete
                    .windows(selected.len())
                    .any(|part| part == selected),
                "every partial view must occupy its original positions: {range:?}"
            );
            // Nested slicing is also position-preserving and borrows storage.
            let nested = view.slice_text(view.text_range()).unwrap();
            assert_eq!(
                nested
                    .positioned_glyphs()
                    .map(glyph_key)
                    .collect::<Vec<_>>(),
                selected
            );
            checked += 1;
        }
    }
    assert!(glyphs.slice_text(0..0).is_none());
    assert!(glyphs.slice_text(usize::MAX..usize::MAX).is_none());
    assert!(glyphs.slice_text(0..usize::MAX).is_none());
    checked
}

#[test]
fn views_reuse_original_glyph_positions_with_bidi_styles_spacing_and_line_breaks() {
    let text = "office a\u{301} العربية AV\nאבג 👩\u{200d}💻 end";
    let split = text.find('ا').unwrap();
    let mut fonts = create_font_context();
    let mut context = LayoutContext::<()>::new();
    let mut checked = 0;
    for direction in [BaseDirection::Ltr, BaseDirection::Rtl] {
        let mut builder = context.style_run_builder(&mut fonts, text, 1.0, false);
        builder.set_base_direction(direction);
        let first = builder.push_style(TextStyle {
            font_family: FontFamily::from(FONT_FAMILY_LIST),
            letter_spacing: 0.75,
            word_spacing: 2.25,
            ..TextStyle::default()
        });
        let second = builder.push_style(TextStyle {
            font_family: FontFamily::from(FONT_FAMILY_LIST),
            font_size: 20.0,
            letter_spacing: -0.25,
            vertical_align: VerticalAlign::SUPER,
            ..TextStyle::default()
        });
        builder.push_style_run(first, ..split);
        builder.push_style_run(second, split..);
        let mut layout = builder.build(text);
        layout.set_text_indent(9.25, IndentOptions::default());
        for width in [None, Some(120.0), Some(83.0)] {
            layout.break_all_lines(width);
            for alignment in [Alignment::Start, Alignment::End, Alignment::Justify] {
                layout.align(alignment, AlignmentOptions::default());
                for line in layout.lines() {
                    for item in line.items() {
                        if let PositionedLayoutItem::GlyphRun(glyphs) = item {
                            checked += check_views(&glyphs);
                        }
                    }
                }
            }
        }
    }
    assert!(checked > 100, "exercise many partial views");
}

#[test]
fn views_reject_offsets_inside_ligatures_combining_sequences_or_utf8_characters() {
    let text = "office a\u{301} العربية 👩\u{200d}💻";
    let mut fonts = create_font_context();
    let mut context = LayoutContext::<()>::new();
    let mut builder = context.ranged_builder(&mut fonts, text, 1.0, false);
    builder.push_default(FontFamily::from(FONT_FAMILY_LIST));
    let mut layout = builder.build(text);
    layout.break_all_lines(None);
    let mut rejected = 0;
    let mut ligature_rejected = 0;
    for line in layout.lines() {
        for item in line.items() {
            let PositionedLayoutItem::GlyphRun(glyphs) = item else {
                continue;
            };
            let range = glyphs.text_range();
            let slice = segment_slice(&glyphs);
            for offset in range.start + 1..range.end {
                let atom = slice.atom_at_text_byte(offset.try_into().unwrap()).unwrap();
                let boundary = slice.text_byte_range(atom.char_range()).start == offset;
                assert_eq!(glyphs.slice_text(offset..range.end).is_some(), boundary);
                assert_eq!(glyphs.slice_text(range.start..offset).is_some(), boundary);
                rejected += usize::from(!boundary);
                if (1..4).contains(&offset) {
                    ligature_rejected += usize::from(!boundary);
                }
            }
            assert!(glyphs.slice_text(range.end..range.start).is_none());
        }
    }
    assert!(rejected > 5, "exercise shape-unsafe and UTF-8 boundaries");
    assert!(ligature_rejected > 0, "exercise a multi-grapheme ligature");
}

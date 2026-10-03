// Copyright 2026 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Bidirectional participation of objects anchored in source text.

use icu_properties::{CodePointMapData, props::BidiMirroringGlyph};
use parlance::{BaseDirection, BidiLevel};

use crate::{Analysis, Analyzer, analysis::AnalysisDataSources};

/// How an object participates in bidirectional analysis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InlineBoxBidi {
    /// Resolve a neutral U+FFFC without adding it to the source text or shaping.
    Neutral,
    /// Inherit the preceding item's assigned level without adding a character.
    InheritPrevious,
    /// Inherit the following participant's level, or the paragraph level at EOF.
    InheritNext,
}

/// An object's source anchor and resolved bidi level.
#[derive(Clone, Copy, Debug)]
pub struct BidiObject {
    /// Byte offset in the source text. Must be a character boundary.
    pub index: usize,
    /// Participation in the Unicode bidirectional algorithm.
    pub participation: InlineBoxBidi,
    /// Assigned level after [`Analyzer::resolve_inline_bidi`].
    pub level: BidiLevel,
}

impl Analyzer {
    /// Resolve text and anchored objects together, retaining source text indices.
    ///
    /// Call after [`Self::analyze`] with the same text and base direction. Objects
    /// must be in stable source order. Neutral objects participate only in UBA;
    /// character info, segmentation, source text and shaping remain unchanged.
    pub fn resolve_inline_bidi(
        &mut self,
        text: &str,
        direction: BaseDirection,
        analysis: &mut Analysis,
        objects: &mut [BidiObject],
    ) {
        let data = AnalysisDataSources::new();
        let brackets = const { CodePointMapData::<BidiMirroringGlyph>::new() };
        let mut characters = text.char_indices().peekable();
        let mut boxes = objects.iter().peekable();
        let inputs = core::iter::from_fn(|| {
            loop {
                let before_character = boxes.peek().is_some_and(|input| {
                    characters
                        .peek()
                        .is_none_or(|(byte, _)| input.index <= *byte)
                });
                let character = if before_character {
                    let input = boxes.next()?;
                    if input.participation != InlineBoxBidi::Neutral {
                        continue;
                    }
                    '\u{fffc}'
                } else {
                    characters.next()?.1
                };
                return Some((
                    character,
                    (
                        data.properties(character).bidi_class(),
                        brackets.get(character),
                    ),
                ));
            }
        });
        self.bidi.resolve(inputs, direction);
        analysis.paragraph_level = self.bidi.base_level();
        analysis.levels.clear();
        analysis.levels.reserve(analysis.info.len());
        let mut levels = self.bidi.levels().iter().copied().peekable();
        let mut boxes = objects.iter_mut().peekable();
        let mut previous = analysis.paragraph_level;
        let assign = |input: &mut BidiObject,
                      previous: &mut BidiLevel,
                      levels: &mut core::iter::Peekable<
            core::iter::Copied<core::slice::Iter<'_, BidiLevel>>,
        >| {
            input.level = match input.participation {
                InlineBoxBidi::Neutral => levels.next().unwrap(),
                InlineBoxBidi::InheritPrevious => *previous,
                InlineBoxBidi::InheritNext => {
                    levels.peek().copied().unwrap_or(analysis.paragraph_level)
                }
            };
            *previous = input.level;
        };
        for (byte, _) in text.char_indices() {
            while boxes.peek().is_some_and(|input| input.index <= byte) {
                assign(boxes.next().unwrap(), &mut previous, &mut levels);
            }
            previous = levels.next().unwrap();
            analysis.levels.push(previous);
        }
        for input in boxes {
            assign(input, &mut previous, &mut levels);
        }
        debug_assert_eq!(
            levels.len(),
            0,
            "all source and virtual levels must be consumed"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AnalysisOptions;

    #[test]
    fn closed_contexts_do_not_leak_into_neutral_objects() {
        let mut analyzer = Analyzer::new();
        let mut analysis = Analysis::new();
        for text in [
            "\u{202e}x\u{202c}",
            "\u{202e}x\u{202c}\n\u{202e}x\u{202c}",
            "\u{202b}x\u{202c}",
            "\u{2067}x\u{2069}",
            "\u{2067}\u{202e}x\u{202c}\u{2069}",
            "😀\u{202e}אב\u{202c}",
        ] {
            analyzer.analyze(text, &AnalysisOptions::default(), &mut analysis);
            let info = analysis.char_info().to_vec();
            let mut objects = [
                BidiObject {
                    index: text.len(),
                    participation: InlineBoxBidi::InheritNext,
                    level: BidiLevel::new(99),
                },
                BidiObject {
                    index: text.len(),
                    participation: InlineBoxBidi::Neutral,
                    level: BidiLevel::new(99),
                },
                BidiObject {
                    index: text.len(),
                    participation: InlineBoxBidi::InheritPrevious,
                    level: BidiLevel::new(99),
                },
            ];
            analyzer.resolve_inline_bidi(text, BaseDirection::Ltr, &mut analysis, &mut objects);
            assert_eq!(analysis.char_info(), info.as_slice(), "{text:?}");
            assert_eq!(
                analysis.bidi_levels().len(),
                text.chars().count(),
                "{text:?}"
            );
            assert!(
                objects
                    .iter()
                    .all(|object| object.level == BidiLevel::new(0)),
                "{text:?}: {objects:?}"
            );
        }
        analyzer.analyze("next", &AnalysisOptions::default(), &mut analysis);
        assert!(
            analysis.bidi_levels().is_empty(),
            "reusing analysis must clear object levels"
        );
        assert_eq!(
            analysis.char_info().len(),
            4,
            "only source characters remain"
        );
    }

    #[test]
    fn objects_inside_overrides_resolve_independently_at_utf8_anchors() {
        let text = "😀\u{202e}אב\u{202c}x";
        let mut analyzer = Analyzer::new();
        let mut analysis = Analysis::new();
        analyzer.analyze(text, &AnalysisOptions::default(), &mut analysis);
        let mut objects = [
            BidiObject {
                index: "😀\u{202e}".len(),
                participation: InlineBoxBidi::Neutral,
                level: BidiLevel::new(0),
            },
            BidiObject {
                index: text.len(),
                participation: InlineBoxBidi::Neutral,
                level: BidiLevel::new(0),
            },
        ];
        analyzer.resolve_inline_bidi(text, BaseDirection::Ltr, &mut analysis, &mut objects);
        assert_eq!(
            objects[0].level,
            BidiLevel::new(1),
            "object inside RTL override"
        );
        assert_eq!(
            objects[1].level,
            BidiLevel::new(0),
            "object outside closed override"
        );
        assert_eq!(
            analysis.bidi_levels().len(),
            text.chars().count(),
            "virtual characters do not leak into text levels"
        );
    }
}

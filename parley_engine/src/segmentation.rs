// Copyright 2026 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Host-provided word and line segmentation.

use alloc::vec::Vec;

use parlance::WordBreak;

/// Provides word and line boundaries using the host application's Unicode data.
///
/// Applications that already bundle a segmenter, such as ICU4C, can implement this trait to
/// reuse its dictionaries instead of enabling Parley's `complex-scripts` feature. The host
/// owns and initializes its segmenter; Parley does not link another copy of ICU.
///
/// This replaces word and line segmentation only. Grapheme segmentation, Unicode properties,
/// normalization, bidi resolution, and mandatory line breaks still use Parley's built-in data.
///
/// Both methods receive an empty buffer whose allocation is reused between calls. Append
/// strictly increasing UTF-8 byte offsets, including `0` and `text.len()` (only `0` for empty
/// text). Every offset must be a character boundary in `text`. A host API that returns UTF-16
/// offsets must convert them to UTF-8 byte offsets before appending them.
///
/// Implementations can be shared between layout contexts and called concurrently.
pub trait TextSegmenter: Send + Sync {
    /// Append all word boundaries, including boundaries around whitespace and punctuation.
    ///
    /// These boundaries are also used for text selection and editing; do not filter them to
    /// include only dictionary words.
    fn word_boundaries(&self, text: &str, boundaries: &mut Vec<usize>);

    /// Append line break opportunities according to `word_break`.
    ///
    /// Offsets are relative to this `text`, which may be a substring of the paragraph when
    /// word-break styles vary. Parley handles the overlap between adjacent style runs and
    /// applies any configured line break override after these boundaries have been collected.
    fn line_boundaries(&self, text: &str, word_break: WordBreak, boundaries: &mut Vec<usize>);
}

#[cfg(test)]
mod tests {
    use core::sync::atomic::{AtomicUsize, Ordering};

    use super::{TextSegmenter, Vec, WordBreak};
    use crate::{Analysis, AnalysisOptions, Analyzer, Boundary};

    struct DictionarySegmenter;

    impl TextSegmenter for DictionarySegmenter {
        fn word_boundaries(&self, text: &str, boundaries: &mut Vec<usize>) {
            assert!(boundaries.is_empty(), "the scratch buffer must be cleared");
            assert_eq!(text, "こんにちは世界");
            boundaries.extend([0, 15, text.len()]);
        }

        fn line_boundaries(&self, text: &str, word_break: WordBreak, boundaries: &mut Vec<usize>) {
            assert!(boundaries.is_empty(), "the scratch buffer must be cleared");
            assert_eq!(word_break, WordBreak::KeepAll);
            boundaries.extend([0, text.len()]);
        }
    }

    #[test]
    fn host_dictionary_boundaries_survive_keep_all_and_analyzer_reuse() {
        let text = "こんにちは世界";
        let mut analyzer = Analyzer::new();
        let mut analysis = Analysis::new();
        let options = AnalysisOptions {
            word_break: &[(0..text.len(), WordBreak::KeepAll)],
            text_segmenter: Some(&DictionarySegmenter),
            ..AnalysisOptions::default()
        };

        for _ in 0..2 {
            analyzer.analyze(text, &options, &mut analysis);
            let words: Vec<_> = text
                .char_indices()
                .zip(analysis.char_info())
                .filter_map(|((offset, _), info)| {
                    (info.boundary == Boundary::Word).then_some(offset)
                })
                .collect();
            assert_eq!(words, [0, 15]);
            assert!(
                analysis
                    .char_info()
                    .iter()
                    .all(|info| info.is_grapheme_start()),
                "host word boundaries must not replace grapheme analysis",
            );
        }

        // Restoring the built-in segmenter must discard previous host boundaries.
        analyzer.analyze(
            "abcdefghijklmnopqrstuvwxyz",
            &AnalysisOptions::default(),
            &mut analysis,
        );
        assert_eq!(analysis.char_info()[0].boundary, Boundary::Word);
        assert!(
            analysis.char_info()[1..]
                .iter()
                .all(|info| info.boundary == Boundary::None),
            "stale host boundaries must not affect the next paragraph",
        );
    }

    struct StyleSegmenter(AtomicUsize);

    impl TextSegmenter for StyleSegmenter {
        fn word_boundaries(&self, text: &str, boundaries: &mut Vec<usize>) {
            boundaries.extend([0, text.len()]);
        }

        fn line_boundaries(&self, text: &str, word_break: WordBreak, boundaries: &mut Vec<usize>) {
            assert!(
                boundaries.is_empty(),
                "style runs must not share boundary contents",
            );
            self.0.fetch_add(1, Ordering::Relaxed);
            match word_break {
                WordBreak::BreakAll => {
                    assert_eq!(text, "é中a");
                    boundaries.extend([0, 2, 5, 6]);
                }
                WordBreak::KeepAll => {
                    assert_eq!(text, "中a界z");
                    boundaries.extend([0, text.len()]);
                }
                WordBreak::Normal => {
                    assert_eq!(text, "a\nb");
                    boundaries.extend([0, text.len()]);
                }
            }
        }
    }

    #[test]
    fn host_line_boundaries_respect_style_overlap_and_utf8_offsets() {
        let segmenter = StyleSegmenter(AtomicUsize::new(0));
        let mut analysis = Analysis::new();
        Analyzer::new().analyze(
            "é中a界z",
            &AnalysisOptions {
                word_break: &[(0..5, WordBreak::BreakAll), (5..10, WordBreak::KeepAll)],
                text_segmenter: Some(&segmenter),
                ..AnalysisOptions::default()
            },
            &mut analysis,
        );

        assert_eq!(segmenter.0.load(Ordering::Relaxed), 2);
        let boundaries: Vec<_> = analysis
            .char_info()
            .iter()
            .map(|info| info.boundary)
            .collect();
        assert_eq!(
            boundaries,
            [
                Boundary::Word,
                Boundary::Line,
                Boundary::None,
                Boundary::None,
                Boundary::None,
            ],
        );
    }

    #[test]
    fn line_overrides_and_mandatory_breaks_apply_with_a_host_segmenter() {
        let segmenter = StyleSegmenter(AtomicUsize::new(0));
        let mut analysis = Analysis::new();
        Analyzer::new().analyze(
            "a\nb",
            &AnalysisOptions {
                text_segmenter: Some(&segmenter),
                line_break_override: Some(&|_| Some(true)),
                ..AnalysisOptions::default()
            },
            &mut analysis,
        );
        let boundaries: Vec<_> = analysis
            .char_info()
            .iter()
            .map(|info| info.boundary)
            .collect();
        assert_eq!(
            boundaries,
            [Boundary::Word, Boundary::Line, Boundary::Mandatory],
        );
        assert_eq!(segmenter.0.load(Ordering::Relaxed), 1);
    }
}

//! Pronunciation lexicon for TTS preprocessing.
//!
//! Substitutes technical terms with phonetic spellings before the text is
//! handed to the TTS provider. Chirp3-HD does support a `custom_pronunciations`
//! API field with IPA/X-SAMPA, but text substitution is provider-agnostic and
//! doesn't require phonetic-alphabet expertise — the tradeoff is that the
//! cached transcript sent to Google diverges slightly from the stored
//! transcript shown to users.
//!
//! Matches are case-sensitive and bounded by non-word characters (roughly
//! `\b<term>\b`), so `Pkl` does not match the `Pkl` inside `Pkls`. Add a
//! separate entry for each inflection you care about.
//!
//! The built-in defaults target terms seen in this project's academic-paper
//! feed; see `default_lexicon()` for the seed list.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LexiconEntry {
    pub term: String,
    pub replacement: String,
}

impl LexiconEntry {
    pub fn new(term: impl Into<String>, replacement: impl Into<String>) -> Self {
        Self {
            term: term.into(),
            replacement: replacement.into(),
        }
    }
}

/// Seed lexicon parsed from `lexicon.txt`. The data file is embedded at
/// compile time — edits require a rebuild, but the TSV is much easier to
/// skim and edit than a Rust literal.
pub fn default_lexicon() -> Vec<LexiconEntry> {
    parse(include_str!("lexicon.txt"))
}

/// Parse the TSV-ish lexicon format: one entry per line, `term\treplacement`,
/// with `#`-prefixed comments and blank lines skipped. Malformed lines panic
/// — the input is a compile-time string, so any error is a bug in the seed
/// file and we want it caught at startup, not silently dropped.
fn parse(src: &str) -> Vec<LexiconEntry> {
    let mut out = Vec::new();
    for (lineno, raw) in src.lines().enumerate() {
        let line = raw.trim_end();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (term, replacement) = line.split_once('\t').unwrap_or_else(|| {
            panic!(
                "lexicon line {}: expected `term<TAB>replacement`, got `{}`",
                lineno + 1,
                line
            )
        });
        let term = term.trim();
        let replacement = replacement.trim();
        if term.is_empty() {
            panic!("lexicon line {}: empty term", lineno + 1);
        }
        out.push(LexiconEntry::new(term, replacement));
    }
    out
}

/// Apply `lexicon` to `text`, returning a new string with whole-word matches
/// replaced. Non-matching regions are copied unchanged.
pub fn apply(text: &str, lexicon: &[LexiconEntry]) -> String {
    if lexicon.is_empty() {
        return text.to_string();
    }
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        // Only attempt a match at a word boundary: either start of text or the
        // previous byte is a non-word char. ASCII-only word definition matches
        // how Google TTS tokenizes these technical terms in practice.
        let at_boundary = i == 0 || !is_word_byte(bytes[i - 1]);
        let mut matched = None;
        if at_boundary {
            for entry in lexicon {
                let term = entry.term.as_bytes();
                if term.is_empty() {
                    continue;
                }
                if bytes[i..].starts_with(term) {
                    let end = i + term.len();
                    let ends_at_boundary = end == bytes.len() || !is_word_byte(bytes[end]);
                    if ends_at_boundary {
                        matched = Some((term.len(), entry.replacement.as_str()));
                        break;
                    }
                }
            }
        }
        if let Some((len, replacement)) = matched {
            out.push_str(replacement);
            i += len;
        } else {
            // Copy one UTF-8 code point so non-ASCII text survives verbatim.
            let ch_len = utf8_char_len(bytes[i]);
            out.push_str(&text[i..i + ch_len]);
            i += ch_len;
        }
    }
    out
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn utf8_char_len(b: u8) -> usize {
    if b < 0x80 {
        1
    } else if b < 0xC0 {
        // Continuation byte — shouldn't start here, but advance one to avoid
        // a loop if the input is somehow malformed.
        1
    } else if b < 0xE0 {
        2
    } else if b < 0xF0 {
        3
    } else {
        4
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lex(pairs: &[(&str, &str)]) -> Vec<LexiconEntry> {
        pairs
            .iter()
            .map(|(t, r)| LexiconEntry::new(*t, *r))
            .collect()
    }

    #[test]
    fn basic_substitution() {
        let l = lex(&[("Coq", "coke")]);
        assert_eq!(apply("I use Coq daily.", &l), "I use coke daily.");
    }

    #[test]
    fn respects_word_boundaries() {
        let l = lex(&[("Pkl", "pickle")]);
        // Must not match inside "Pkls" (plural) or "xPkl".
        assert_eq!(apply("Pkls and xPkl", &l), "Pkls and xPkl");
    }

    #[test]
    fn boundary_against_punctuation() {
        let l = lex(&[("CEL", "sell")]);
        assert_eq!(apply("(CEL) and CEL, too.", &l), "(sell) and sell, too.");
    }

    #[test]
    fn case_sensitive() {
        let l = lex(&[("Coq", "coke")]);
        // Lowercase "coq" should not match — user adds an explicit entry if they want it.
        assert_eq!(apply("coq and Coq", &l), "coq and coke");
    }

    #[test]
    fn no_overlap_after_replacement() {
        // Replacement text itself must not be re-scanned; otherwise "Pkl" -> "Pkl"
        // loops or a replacement containing a shorter term double-expands.
        let l = lex(&[("A", "AA")]);
        assert_eq!(apply("A", &l), "AA");
    }

    #[test]
    fn multiple_terms_first_wins() {
        let l = lex(&[("NoSQL", "no sequel"), ("SQL", "sequel")]);
        // "NoSQL" must take precedence over "SQL" when both match at the same
        // position would otherwise both apply.
        assert_eq!(apply("NoSQL and SQL", &l), "no sequel and sequel");
    }

    #[test]
    fn empty_lexicon_passthrough() {
        assert_eq!(apply("anything goes", &[]), "anything goes");
    }

    #[test]
    fn preserves_non_ascii() {
        let l = lex(&[("Coq", "coke")]);
        assert_eq!(apply("Coq — café", &l), "coke — café");
    }

    #[test]
    fn default_lexicon_has_entries() {
        let l = default_lexicon();
        assert!(!l.is_empty());
        // Spot-check a representative entry so an accidental deletion trips CI.
        assert!(l.iter().any(|e| e.term == "Coq"));
    }

    #[test]
    fn parse_skips_comments_and_blank_lines() {
        let src = "# a comment\n\nCoq\tcock\n   \n# trailing comment\nMIR\tem eye are\n";
        let l = parse(src);
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].term, "Coq");
        assert_eq!(l[0].replacement, "cock");
        assert_eq!(l[1].term, "MIR");
        assert_eq!(l[1].replacement, "em eye are");
    }

    #[test]
    #[should_panic(expected = "expected `term<TAB>replacement`")]
    fn parse_rejects_missing_tab() {
        parse("Coq coke\n");
    }
}

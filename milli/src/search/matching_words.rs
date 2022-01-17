use std::cmp::{min, Reverse};
use std::collections::{BTreeMap, HashSet};
use std::ops::{Index, IndexMut};

use levenshtein_automata::{Distance, DFA};
use meilisearch_tokenizer::Token;

use super::build_dfa;
use crate::search::query_tree::{Operation, Query};

type IsPrefix = bool;

/// Structure created from a query tree
/// referencing words that match the given query tree.
#[derive(Default)]
pub struct MatchingWords {
    dfas: Vec<(DFA, String, u8, IsPrefix)>,
}

impl MatchingWords {
    pub fn from_query_tree(tree: &Operation) -> Self {
        // fetch matchable words from the query tree
        let mut dfas: Vec<_> = fetch_queries(tree)
            .into_iter()
            // create DFAs for each word
            .map(|(w, t, p)| (build_dfa(w, t, p), w.to_string(), t, p))
            .collect();
        // Sort word by len in DESC order prioritizing the longuest word,
        // in order to highlight the longuest part of the matched word.
        dfas.sort_unstable_by_key(|(_dfa, query_word, _typo, _is_prefix)| {
            Reverse(query_word.len())
        });
        Self { dfas }
    }

    /// Returns the number of matching bytes if the word matches one of the query words.
    pub fn matching_bytes(&self, word_to_highlight: &Token) -> Option<usize> {
        self.dfas.iter().find_map(|(dfa, query_word, typo, is_prefix)| {
            match dfa.eval(word_to_highlight.text()) {
                Distance::Exact(t) if t <= *typo => {
                    if *is_prefix {
                        let len = bytes_to_highlight(word_to_highlight.text(), query_word);
                        Some(word_to_highlight.num_chars_from_bytes(len))
                    } else {
                        Some(word_to_highlight.num_chars_from_bytes(word_to_highlight.text().len()))
                    }
                }
                _otherwise => None,
            }
        })
    }
}

/// Lists all words which can be considered as a match for the query tree.
fn fetch_queries(tree: &Operation) -> HashSet<(&str, u8, IsPrefix)> {
    fn resolve_ops<'a>(tree: &'a Operation, out: &mut HashSet<(&'a str, u8, IsPrefix)>) {
        match tree {
            Operation::Or(_, ops) | Operation::And(ops) => {
                ops.as_slice().iter().for_each(|op| resolve_ops(op, out));
            }
            Operation::Query(Query { prefix, kind }) => {
                let typo = if kind.is_exact() { 0 } else { kind.typo() };
                out.insert((kind.word(), typo, *prefix));
            }
            Operation::Phrase(words) => {
                for word in words {
                    out.insert((word, 0, false));
                }
            }
        }
    }

    let mut queries = HashSet::new();
    resolve_ops(tree, &mut queries);
    queries
}

// A simple wrapper around vec so we can get contiguous but index it like it's 2D array.
struct N2Array<T> {
    y_size: usize,
    buf: Vec<T>,
}

impl<T: Clone> N2Array<T> {
    fn new(x: usize, y: usize, value: T) -> N2Array<T> {
        N2Array { y_size: y, buf: vec![value; x * y] }
    }
}

impl<T> Index<(usize, usize)> for N2Array<T> {
    type Output = T;

    #[inline]
    fn index(&self, (x, y): (usize, usize)) -> &T {
        &self.buf[(x * self.y_size) + y]
    }
}

impl<T> IndexMut<(usize, usize)> for N2Array<T> {
    #[inline]
    fn index_mut(&mut self, (x, y): (usize, usize)) -> &mut T {
        &mut self.buf[(x * self.y_size) + y]
    }
}

/// Returns the number of **bytes** we want to highlight in the `source` word.
/// Basically we want to highlight as much characters as possible in the source until it has too much
/// typos (= 2)
/// The algorithm is a modified
/// [Damerau-Levenshtein](https://en.wikipedia.org/wiki/Damerau%E2%80%93Levenshtein_distance)
fn bytes_to_highlight(source: &str, target: &str) -> usize {
    let n = source.chars().count();
    let m = target.chars().count();

    if n == 0 {
        return 0;
    }
    // since we allow two typos we can send two characters even if it's completely wrong
    if m < 3 {
        return source.chars().take(m).map(|c| c.len_utf8()).sum();
    }
    if n == m && source == target {
        return source.len();
    }

    let inf = n + m;
    let mut matrix = N2Array::new(n + 2, m + 2, 0);

    matrix[(0, 0)] = inf;
    for i in 0..=n {
        matrix[(i + 1, 0)] = inf;
        matrix[(i + 1, 1)] = i;
    }
    for j in 0..=m {
        matrix[(0, j + 1)] = inf;
        matrix[(1, j + 1)] = j;
    }

    let mut last_row = BTreeMap::new();

    for (row, char_s) in source.chars().enumerate() {
        let mut last_match_col = 0;
        let row = row + 1;

        for (col, char_t) in target.chars().enumerate() {
            let col = col + 1;
            let last_match_row = *last_row.get(&char_t).unwrap_or(&0);
            let cost = if char_s == char_t { 0 } else { 1 };

            let dist_add = matrix[(row, col + 1)] + 1;
            let dist_del = matrix[(row + 1, col)] + 1;
            let dist_sub = matrix[(row, col)] + cost;
            let dist_trans = matrix[(last_match_row, last_match_col)]
                + (row - last_match_row - 1)
                + 1
                + (col - last_match_col - 1);
            let dist = min(min(dist_add, dist_del), min(dist_sub, dist_trans));
            matrix[(row + 1, col + 1)] = dist;

            if cost == 0 {
                last_match_col = col;
            }
        }

        last_row.insert(char_s, row);
    }

    let mut minimum = (u32::max_value(), 0);
    for x in 0..=m {
        let dist = matrix[(n + 1, x + 1)] as u32;
        if dist < minimum.0 {
            minimum = (dist, x);
        }
    }

    // everything was done characters wise and now we want to returns a number of bytes
    source.chars().take(minimum.1).map(|c| c.len_utf8()).sum()
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::str::from_utf8;

    use meilisearch_tokenizer::TokenKind;

    use super::*;
    use crate::search::query_tree::{Operation, Query, QueryKind};
    use crate::MatchingWords;

    #[test]
    fn test_bytes_to_highlight() {
        struct TestBytesToHighlight {
            query: &'static str,
            text: &'static str,
            length: usize,
        }
        let tests = [
            TestBytesToHighlight { query: "bip", text: "bip", length: "bip".len() },
            TestBytesToHighlight { query: "bip", text: "boup", length: "bip".len() },
            TestBytesToHighlight {
                query: "Levenshtein",
                text: "Levenshtein",
                length: "Levenshtein".len(),
            },
            // we get to the end of our word with only one typo
            TestBytesToHighlight {
                query: "Levenste",
                text: "Levenshtein",
                length: "Levenste".len(),
            },
            // we get our third and last authorized typo right on the last character
            TestBytesToHighlight {
                query: "Levenstein",
                text: "Levenshte",
                length: "Levenste".len(),
            },
            // we get to the end of our word with only two typos at the beginning
            TestBytesToHighlight {
                query: "Bavenshtein",
                text: "Levenshtein",
                length: "Bavenshtein".len(),
            },
            TestBytesToHighlight {
                query: "Альфа", text: "Альфой", length: "Альф".len()
            },
            TestBytesToHighlight {
                query: "Go💼", text: "Go💼od luck.", length: "Go💼".len()
            },
            TestBytesToHighlight {
                query: "Go💼od", text: "Go💼od luck.", length: "Go💼od".len()
            },
            TestBytesToHighlight {
                query: "chäräcters",
                text: "chäräcters",
                length: "chäräcters".len(),
            },
            TestBytesToHighlight { query: "ch", text: "chäräcters", length: "ch".len() },
            TestBytesToHighlight { query: "chär", text: "chäräcters", length: "chär".len() },
        ];

        for test in &tests {
            let length = bytes_to_highlight(test.text, test.query);
            assert_eq!(length, test.length, r#"lenght between: "{}" "{}""#, test.query, test.text);
            assert!(
                from_utf8(&test.query.as_bytes()[..length]).is_ok(),
                r#"converting {}[..{}] to an utf8 str failed"#,
                test.query,
                length
            );
        }
    }

    #[test]
    fn matching_words() {
        let query_tree = Operation::Or(
            false,
            vec![Operation::And(vec![
                Operation::Query(Query {
                    prefix: true,
                    kind: QueryKind::exact("split".to_string()),
                }),
                Operation::Query(Query {
                    prefix: false,
                    kind: QueryKind::exact("this".to_string()),
                }),
                Operation::Query(Query {
                    prefix: true,
                    kind: QueryKind::tolerant(1, "world".to_string()),
                }),
            ])],
        );

        let matching_words = MatchingWords::from_query_tree(&query_tree);

        assert_eq!(
            matching_words.matching_bytes(&Token {
                kind: TokenKind::Word,
                word: Cow::Borrowed("word"),
                byte_start: 0,
                char_index: 0,
                byte_end: "word".len(),
                char_map: None,
            }),
            Some(3)
        );
        assert_eq!(
            matching_words.matching_bytes(&Token {
                kind: TokenKind::Word,
                word: Cow::Borrowed("nyc"),
                byte_start: 0,
                char_index: 0,
                byte_end: "nyc".len(),
                char_map: None,
            }),
            None
        );
        assert_eq!(
            matching_words.matching_bytes(&Token {
                kind: TokenKind::Word,
                word: Cow::Borrowed("world"),
                byte_start: 0,
                char_index: 0,
                byte_end: "world".len(),
                char_map: None,
            }),
            Some(5)
        );
        assert_eq!(
            matching_words.matching_bytes(&Token {
                kind: TokenKind::Word,
                word: Cow::Borrowed("splitted"),
                byte_start: 0,
                char_index: 0,
                byte_end: "splitted".len(),
                char_map: None,
            }),
            Some(5)
        );
        assert_eq!(
            matching_words.matching_bytes(&Token {
                kind: TokenKind::Word,
                word: Cow::Borrowed("thisnew"),
                byte_start: 0,
                char_index: 0,
                byte_end: "thisnew".len(),
                char_map: None,
            }),
            None
        );
        assert_eq!(
            matching_words.matching_bytes(&Token {
                kind: TokenKind::Word,
                word: Cow::Borrowed("borld"),
                byte_start: 0,
                char_index: 0,
                byte_end: "borld".len(),
                char_map: None,
            }),
            Some(5)
        );
        assert_eq!(
            matching_words.matching_bytes(&Token {
                kind: TokenKind::Word,
                word: Cow::Borrowed("wordsplit"),
                byte_start: 0,
                char_index: 0,
                byte_end: "wordsplit".len(),
                char_map: None,
            }),
            Some(4)
        );
    }
}

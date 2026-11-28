use std::collections::HashMap;

use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

pub fn normalize_name(name: &str) -> String {
    let decomposed: String = name
        .nfkd()
        .filter(|c| !is_combining_mark(*c))
        .flat_map(char::to_lowercase)
        .map(|c| if c == '\u{0671}' { '\u{0627}' } else { c })
        .collect();
    decomposed.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Match and rank names with the browser's Fuse configuration.
pub fn rank_by_name<T>(
    values: impl IntoIterator<Item = T>,
    query: &str,
    name: impl Fn(&T) -> &str,
) -> Vec<T> {
    let term: Vec<_> = normalize_name(query).encode_utf16().collect();
    let values = values.into_iter();
    if term.is_empty() {
        return values.collect();
    }
    let (full, remainder) = term.split_at(term.len() / 32 * 32);
    let mut chunks: Vec<_> = full.chunks(32).collect();
    if !remainder.is_empty() {
        chunks.push(&term[term.len().saturating_sub(32)..]);
    }
    let mut matches: Vec<_> = values
        .filter_map(|value| name_score(name(&value), &term, &chunks).map(|score| (score, value)))
        .collect();
    matches.sort_by(|left, right| left.0.total_cmp(&right.0));
    matches.into_iter().map(|(_, value)| value).collect()
}

// Fuse's default Bitap scoring with ignoreLocation=true and a single name key.
// Match on UTF-16 units to preserve the JavaScript matcher and its 32-bit chunks.
fn name_score(name: &str, term: &[u16], chunks: &[&[u16]]) -> Option<f64> {
    let name = normalize_name(name);
    let text: Vec<_> = name.encode_utf16().collect();
    let threshold = if term.len() < 3 { 0.0 } else { 0.34 };
    let score = if text == term {
        f64::EPSILON
    } else {
        let scores: Vec<_> = chunks
            .iter()
            .map(|chunk| bitap(&text, chunk, threshold))
            .collect();
        if scores.iter().all(Option::is_none) {
            return None;
        }
        scores.iter().map(|score| score.unwrap_or(1.0)).sum::<f64>() / scores.len() as f64
    };
    let norm = (1000.0 / (name.split_whitespace().count() as f64).sqrt()).round() / 1000.0;
    Some(score.powf(norm))
}

fn bitap(text: &[u16], pattern: &[u16], threshold: f64) -> Option<f64> {
    if text.windows(pattern.len()).any(|word| word == pattern) {
        return Some(0.001);
    }
    let mut masks = HashMap::<u16, u32>::new();
    for (index, &c) in pattern.iter().enumerate() {
        *masks.entry(c).or_default() |= 1 << (pattern.len() - index - 1);
    }
    let high_bit = 1 << (pattern.len() - 1);
    let end = text.len() + pattern.len();
    let mut previous = vec![0_u32; end + 2];
    let mut current = previous.clone();
    for errors in 0..=(pattern.len() as f64 * threshold) as u32 {
        current[end + 1] = (1 << errors) - 1;
        for index in (1..=end).rev() {
            let mask = text
                .get(index - 1)
                .and_then(|c| masks.get(c))
                .copied()
                .unwrap_or(0);
            current[index] = ((current[index + 1] << 1) | 1) & mask;
            if errors > 0 {
                current[index] |=
                    ((previous[index + 1] | previous[index]) << 1) | 1 | previous[index + 1];
            }
            if current[index] & high_bit != 0 {
                return Some((f64::from(errors) / pattern.len() as f64).max(0.001));
            }
        }
        std::mem::swap(&mut previous, &mut current);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks_fuzzy_names_and_normalizes_arabic_and_accents() {
        let names = [
            "One Recitation",
            "Speech",
            "Recitation",
            "Marc Bélanger",
            "أَحْمَد",
        ];
        assert_eq!(
            rank_by_name(names, "Recittion", |name| name),
            ["Recitation", "One Recitation"]
        );
        assert_eq!(
            rank_by_name(names, "marc belanger", |name| name),
            ["Marc Bélanger"]
        );
        assert_eq!(rank_by_name(names, "احمد", |name| name), ["أَحْمَد"]);
        assert_eq!(rank_by_name(names, "", |name| name), names);
    }
}

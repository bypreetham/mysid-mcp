//! "Did you mean ...?" for a mistyped command.


use crate::dispatch::*;

pub(crate) fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i];
        for j in 1..=b.len() {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            cur.push((prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// Up to three known commands the user probably meant: prefixes of what they typed, or close spellings.
pub(crate) fn suggest_commands(unknown: &str) -> Vec<&'static str> {
    let u = unknown.trim().trim_start_matches('-').to_ascii_lowercase();
    if u.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(usize, &'static str)> = KNOWN_COMMANDS
        .iter()
        .filter_map(|c| {
            let d = edit_distance(&u, c);
            let prefix = u.len() >= 3 && (c.starts_with(u.as_str()) || u.starts_with(c));
            if prefix { Some((0, *c)) } else if d <= 2 { Some((d, *c)) } else { None }
        })
        .collect();
    scored.sort();
    // Only the closest matches: `bild` means `build`, not also the next-nearest spelling.
    let best = scored.first().map_or(0, |s| s.0);
    scored.into_iter().filter(|s| s.0 == best).map(|(_, c)| c).take(3).collect()
}

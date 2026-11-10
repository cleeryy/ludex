//! Fuzzy title search and type-ahead suggestions.
//!
//! Matching is diacritic- and punctuation-insensitive. Titles are scored with a
//! small cascade (exact, prefix, substring, token prefix, trigram similarity,
//! Levenshtein ratio) so a typo such as `spidrman` still finds
//! *Marvel's Spider-Man 2* while unrelated rows stay out. The library is small
//! enough (thousands of rows) that scoring happens in Rust after the SQL
//! filters have narrowed the candidate set.

use axum::routing::get;
use axum::{extract::State, Json, Router};

use crate::auth::AuthUser;
use crate::db;
use crate::error::ApiError;
use crate::extract::ApiQuery;
use crate::models::{GameSuggestion, SuggestQuery, SuggestResponse};
use crate::state::AppState;

/// Minimum score a candidate needs to be considered a match.
const MIN_SCORE: f64 = 300.0;

/// One field match for a game.
#[derive(Debug, Clone)]
pub struct TitleMatch {
    /// Score between 0 and 1000.
    pub score: f64,
    /// Field that matched: `title`, `sortTitle` or `altTitle`.
    pub field: &'static str,
    /// Text that matched (display title or alias).
    pub text: String,
}

/// Search routes.
pub fn router() -> Router<AppState> {
    Router::new().route("/games/suggest", get(suggest))
}

#[derive(Debug, sqlx::FromRow)]
struct SuggestRow {
    id: i64,
    title: String,
    sort_title: String,
    release_date: Option<String>,
    cover_path: Option<String>,
    alt_titles_json: String,
}

async fn suggest(
    State(state): State<AppState>,
    _user: AuthUser,
    ApiQuery(query): ApiQuery<SuggestQuery>,
) -> Result<Json<SuggestResponse>, ApiError> {
    let raw = query.q.as_deref().unwrap_or("").trim();
    if raw.is_empty() {
        return Ok(Json(SuggestResponse {
            query: String::new(),
            suggestions: Vec::new(),
        }));
    }
    let limit = query.limit.unwrap_or(8).clamp(1, 20);
    let normalized = normalize(raw);

    let rows = sqlx::query_as::<_, SuggestRow>(
        "SELECT id, title, sort_title, release_date, cover_path, alt_titles_json
         FROM games ORDER BY sort_title ASC LIMIT 5000",
    )
    .fetch_all(&state.pool)
    .await
    .map_err(ApiError::from)?;

    let mut scored: Vec<(f64, SuggestRow, TitleMatch)> = rows
        .into_iter()
        .filter_map(|row| {
            let alt_titles = db::parse_json_list(&row.alt_titles_json);
            let found = match_game(raw, &row.title, &row.sort_title, &alt_titles)?;
            Some((found.score, row, found))
        })
        .collect();
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.sort_title.cmp(&b.1.sort_title))
    });

    let suggestions = scored
        .into_iter()
        .take(limit as usize)
        .map(|(score, row, found)| GameSuggestion {
            id: row.id,
            title: row.title,
            year: row
                .release_date
                .as_deref()
                .and_then(|value| value.get(0..4))
                .and_then(|value| value.parse::<i32>().ok()),
            cover_url: row.cover_path.as_deref().map(crate::media::media_url),
            matched_field: found.field.to_string(),
            matched_title: found.text,
            score: (score * 10.0).round() / 10.0,
        })
        .collect();

    Ok(Json(SuggestResponse {
        query: normalized,
        suggestions,
    }))
}

/// Scores a game against a query across its title, sort title and aliases.
#[must_use]
pub fn match_game(
    query: &str,
    title: &str,
    sort_title: &str,
    alt_titles: &[String],
) -> Option<TitleMatch> {
    let normalized_query = normalize(query);
    if normalized_query.is_empty() {
        return None;
    }

    let mut best: Option<TitleMatch> = None;
    let mut consider = |field: &'static str, text: &str| {
        let candidate = normalize(text);
        let Some(score) = score_normalized(&normalized_query, &candidate) else {
            return;
        };
        if score < MIN_SCORE {
            return;
        }
        let better = best
            .as_ref()
            .is_none_or(|current| score > current.score + 0.5);
        if better {
            best = Some(TitleMatch {
                score,
                field,
                text: text.to_string(),
            });
        }
    };

    consider("title", title);
    if sort_title != title {
        consider("sortTitle", sort_title);
    }
    for alias in alt_titles {
        consider("altTitle", alias);
    }
    best
}

/// Lowercases, folds diacritics and collapses punctuation into single spaces.
#[must_use]
pub fn normalize(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut pending_space = false;
    for character in input.chars() {
        for lower in character.to_lowercase() {
            let ch = fold_char(lower);
            if ch.is_alphanumeric() {
                if pending_space && !out.is_empty() {
                    out.push(' ');
                }
                pending_space = false;
                out.push(ch);
            } else {
                pending_space = true;
            }
        }
    }
    out
}

fn fold_char(ch: char) -> char {
    match ch {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => 'a',
        'ç' | 'ć' | 'č' => 'c',
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ė' | 'ę' => 'e',
        'ì' | 'í' | 'î' | 'ï' | 'ī' | 'į' => 'i',
        'ñ' | 'ń' | 'ň' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ő' => 'o',
        'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ů' | 'ű' => 'u',
        'ý' | 'ÿ' => 'y',
        'ß' => 's',
        'æ' => 'e',
        'œ' => 'e',
        'š' => 's',
        'ž' => 'z',
        'ł' => 'l',
        'đ' => 'd',
        'þ' => 't',
        other => other,
    }
}

fn score_normalized(query: &str, candidate: &str) -> Option<f64> {
    if query.is_empty() || candidate.is_empty() {
        return None;
    }
    if query == candidate {
        return Some(1000.0);
    }
    let query_len = query.chars().count() as f64;
    let candidate_len = candidate.chars().count() as f64;

    if candidate.starts_with(query) {
        return Some(920.0 - (candidate_len - query_len).min(60.0));
    }

    if let Some(byte_index) = candidate.find(query) {
        let prefix = &candidate[..byte_index];
        let at_boundary = prefix.is_empty() || prefix.ends_with(' ');
        let position = prefix.chars().count() as f64;
        let base = if at_boundary { 870.0 } else { 820.0 };
        return Some(base - position.min(40.0) * 0.5 - (candidate_len - query_len).min(60.0) * 0.2);
    }

    let query_tokens: Vec<&str> = query.split(' ').collect();
    if query_tokens.len() > 1 {
        let candidate_tokens: Vec<&str> = candidate.split(' ').collect();
        if query_tokens
            .iter()
            .all(|token| candidate_tokens.iter().any(|word| word.starts_with(token)))
        {
            return Some(780.0 - (candidate_len - query_len).min(60.0) * 0.2);
        }
    }

    let dice = trigram_similarity(query, candidate);
    if dice >= 0.40 {
        return Some(420.0 + dice * 380.0);
    }

    let distance = levenshtein(query, candidate);
    let longest = query_len.max(candidate_len);
    let ratio = 1.0 - distance as f64 / longest;
    if ratio >= 0.60 {
        return Some(240.0 + ratio * 420.0);
    }

    // A typo inside one word of a longer title: compare against every word and
    // every two-word window, then keep the best ratio.
    if query_len >= 3.0 {
        let best = best_window_ratio(query, candidate);
        if best >= 0.72 {
            return Some(300.0 + best * 320.0);
        }
    }

    None
}

fn best_window_ratio(query: &str, candidate: &str) -> f64 {
    let words: Vec<&str> = candidate
        .split(' ')
        .filter(|word| !word.is_empty())
        .collect();
    let mut best: f64 = 0.0;
    for size in 1..=2 {
        for start in 0..words.len() {
            let end = (start + size).min(words.len());
            if start >= end {
                continue;
            }
            let window = words[start..end].join(" ");
            let distance = levenshtein(query, &window);
            let longest = query.chars().count().max(window.chars().count());
            if longest == 0 {
                continue;
            }
            let ratio = 1.0 - distance as f64 / longest as f64;
            if ratio > best {
                best = ratio;
            }
        }
    }
    best
}

fn trigram_similarity(a: &str, b: &str) -> f64 {
    let left = trigrams(a);
    let right = trigrams(b);
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    let shared = left.intersection(&right).count() as f64;
    2.0 * shared / (left.len() + right.len()) as f64
}

fn trigrams(value: &str) -> std::collections::HashSet<String> {
    let padded = format!(" {value} ");
    let chars: Vec<char> = padded.chars().collect();
    let mut set = std::collections::HashSet::new();
    if chars.len() < 3 {
        set.insert(padded);
        return set;
    }
    for window in chars.windows(3) {
        set.insert(window.iter().collect());
    }
    set
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0usize; b.len() + 1];
    for (i, a_char) in a.iter().enumerate() {
        current[0] = i + 1;
        for (j, b_char) in b.iter().enumerate() {
            let cost = usize::from(a_char != b_char);
            current[j + 1] = (previous[j] + cost)
                .min(previous[j + 1] + 1)
                .min(current[j] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[b.len()]
}

/// Applies fuzzy scoring to a list of rows, returning ranked matches.
pub fn rank_rows<T, F>(query: &str, rows: Vec<T>, mut fields: F) -> Vec<(T, TitleMatch)>
where
    F: FnMut(&T) -> (String, String, Vec<String>),
{
    let mut scored: Vec<(T, TitleMatch)> = rows
        .into_iter()
        .filter_map(|row| {
            let (title, sort_title, alt_titles) = fields(&row);
            let found = match_game(query, &title, &sort_title, &alt_titles)?;
            Some((row, found))
        })
        .collect();
    scored.sort_by(|a, b| {
        b.1.score
            .partial_cmp(&a.1.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    scored
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalises_accents_and_punctuation() {
        assert_eq!(normalize("Marvel's Spider-Man 2"), "marvel s spider man 2");
        assert_eq!(normalize("Pokémon Édition"), "pokemon edition");
        assert_eq!(normalize("  ...  "), "");
    }

    #[test]
    fn exact_and_prefix_rank_above_typos() {
        let exact = match_game("hades", "Hades", "hades", &[]).expect("exact");
        let prefix = match_game("had", "Hades", "hades", &[]).expect("prefix");
        let typo = match_game("hadse", "Hades", "hades", &[]).expect("typo");
        assert_eq!(exact.field, "title");
        assert!(
            exact.score > prefix.score,
            "{} {}",
            exact.score,
            prefix.score
        );
        assert!(prefix.score > typo.score, "{} {}", prefix.score, typo.score);
    }

    #[test]
    fn typos_inside_long_titles_match() {
        let found = match_game(
            "spidrman",
            "Marvel's Spider-Man 2",
            "marvel s spider man 2",
            &[],
        )
        .expect("typo match");
        assert_eq!(found.field, "title");
        assert!(found.score >= MIN_SCORE);
    }

    #[test]
    fn accents_are_ignored_in_both_directions() {
        assert!(match_game("pokemon", "Pokémon", "pokemon", &[]).is_some());
        assert!(match_game("pokémon", "Pokemon", "pokemon", &[]).is_some());
    }

    #[test]
    fn aliases_match_and_report_the_alias() {
        let found = match_game(
            "spider man",
            "Marvel's Spider-Man 2",
            "marvel s spider man 2",
            &[],
        )
        .expect("match");
        assert_eq!(found.field, "title");

        let found = match_game(
            "webhead chronicles",
            "Marvel's Spider-Man 2",
            "marvel s spider man 2",
            &["Webhead Chronicles".to_string()],
        )
        .expect("alias match");
        assert_eq!(found.field, "altTitle");
        assert_eq!(found.text, "Webhead Chronicles");
    }

    #[test]
    fn unrelated_queries_do_not_match() {
        assert!(match_game("zelda", "Hades", "hades", &[]).is_none());
        assert!(match_game("zzzzzz", "Celeste", "celeste", &[]).is_none());
    }

    #[test]
    fn ranking_puts_best_first() {
        let rows = vec!["Celeste", "Celestial Tides", "Hades"];
        let ranked = rank_rows("celest", rows, |title| {
            (title.to_string(), title.to_lowercase(), Vec::new())
        });
        assert_eq!(ranked[0].0, "Celeste");
        assert!(ranked.len() >= 2);
    }
}

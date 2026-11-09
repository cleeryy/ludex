//! Slug generation and ASCII folding helpers.
//!
//! Slugs are used for install folders (`{installDir}/{slug}/{version}`) and for
//! stable identifiers derived from a title. They are lowercase ASCII, use `-`
//! as the only separator and never exceed 80 characters.

/// Maximum slug length, in characters.
pub const MAX_SLUG_LEN: usize = 80;

/// Folds a single Latin character to its ASCII equivalent.
///
/// Returns `None` when the character has no known ASCII fold. Callers decide
/// whether to keep the character as-is (non-Latin scripts) or drop it.
pub(crate) fn fold_to_ascii(c: char) -> Option<&'static str> {
    let folded = match c {
        'a' => "a",
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => "a",
        'æ' => "ae",
        'b' => "b",
        'c' => "c",
        'ç' | 'ć' | 'č' | 'ĉ' | 'ċ' => "c",
        'd' => "d",
        'ď' | 'đ' | 'ð' => "d",
        'e' => "e",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => "e",
        'f' => "f",
        'g' => "g",
        'ğ' | 'ĝ' | 'ġ' | 'ģ' => "g",
        'h' => "h",
        'ĥ' | 'ħ' => "h",
        'i' => "i",
        'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'į' | 'ı' => "i",
        'j' => "j",
        'ĵ' => "j",
        'k' => "k",
        'ķ' => "k",
        'l' => "l",
        'ĺ' | 'ļ' | 'ľ' | 'ł' => "l",
        'm' => "m",
        'n' => "n",
        'ñ' | 'ń' | 'ņ' | 'ň' => "n",
        'o' => "o",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => "o",
        'œ' => "oe",
        'p' => "p",
        'q' => "q",
        'r' => "r",
        'ŕ' | 'ŗ' | 'ř' => "r",
        's' => "s",
        'ś' | 'ŝ' | 'ş' | 'š' => "s",
        'ß' => "ss",
        't' => "t",
        'ţ' | 'ť' | 'ŧ' => "t",
        'u' => "u",
        'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => "u",
        'v' => "v",
        'w' => "w",
        'ŵ' => "w",
        'x' => "x",
        'y' => "y",
        'ý' | 'ÿ' | 'ŷ' => "y",
        'z' => "z",
        'ź' | 'ż' | 'ž' => "z",
        'þ' => "th",
        _ => return None,
    };
    Some(folded)
}

/// Converts a title into a URL/folder-safe slug.
///
/// The result is lowercase, ASCII-folded for common Latin accents, uses `-`
/// between words, has no leading/trailing or repeated `-`, and is at most
/// [`MAX_SLUG_LEN`] characters long.
///
/// ```
/// assert_eq!(ludex_core::slugify("Clair Obscur: Expedition 33"), "clair-obscur-expedition-33");
/// assert_eq!(ludex_core::slugify("Pokémon Écarlate"), "pokemon-ecarlate");
/// ```
pub fn slugify(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for c in input.chars() {
        for lower in c.to_lowercase() {
            if lower.is_ascii_alphanumeric() {
                out.push(lower);
            } else if let Some(folded) = fold_to_ascii(lower) {
                out.push_str(folded);
            } else if lower.is_alphanumeric() {
                // Keep non-Latin letters (CJK, Cyrillic, ...) rather than
                // producing an empty slug for non-English titles.
                out.push(lower);
            } else if !out.ends_with('-') {
                out.push('-');
            }
        }
    }
    let trimmed = out.trim_matches('-');
    let capped: String = trimmed.chars().take(MAX_SLUG_LEN).collect();
    capped.trim_end_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowercases_and_separates() {
        assert_eq!(slugify("Blue Prince"), "blue-prince");
        assert_eq!(
            slugify("Clair Obscur: Expedition 33"),
            "clair-obscur-expedition-33"
        );
        assert_eq!(
            slugify("UNCHARTED Legacy of Thieves Collection"),
            "uncharted-legacy-of-thieves-collection"
        );
    }

    #[test]
    fn folds_accents() {
        assert_eq!(slugify("Pokémon Écarlate"), "pokemon-ecarlate");
        assert_eq!(slugify("Æon Flux"), "aeon-flux");
        assert_eq!(slugify("Straße"), "strasse");
        assert_eq!(slugify("Don't Starve Together"), "don-t-starve-together");
    }

    #[test]
    fn collapses_and_trims_separators() {
        assert_eq!(slugify("  --Hello,,,   World!!  "), "hello-world");
        assert_eq!(slugify("..."), "");
        assert_eq!(slugify(""), "");
    }

    #[test]
    fn caps_length() {
        let long = "a".repeat(200);
        let slug = slugify(&long);
        assert_eq!(slug.chars().count(), MAX_SLUG_LEN);
        assert!(slug.chars().all(|c| c == 'a'));
    }

    #[test]
    fn keeps_non_latin_letters() {
        assert_eq!(slugify("塞尔达传说"), "塞尔达传说");
    }
}

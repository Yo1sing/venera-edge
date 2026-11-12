use std::{collections::HashMap, sync::OnceLock};

type Dictionaries = (HashMap<char, char>, HashMap<char, char>);
static DICTIONARIES: OnceLock<Dictionaries> = OnceLock::new();

fn dictionaries() -> &'static Dictionaries {
    DICTIONARIES.get_or_init(|| {
        let mut simplified = HashMap::new();
        let mut traditional = HashMap::new();
        for line in include_str!("../../assets/opencc.txt").lines() {
            let chars: Vec<char> = line.chars().collect();
            if line.starts_with('#') || chars.len() != 2 {
                continue;
            }
            simplified.insert(chars[0], chars[1]);
            traditional.insert(chars[1], chars[0]);
        }
        (simplified, traditional)
    })
}

pub fn convert(text: &str, to_simplified: bool) -> String {
    let (simplified, traditional) = dictionaries();
    let map = if to_simplified {
        traditional
    } else {
        simplified
    };
    text.chars().map(|c| *map.get(&c).unwrap_or(&c)).collect()
}

pub fn contains(text: &str, traditional: bool) -> bool {
    let (s, t) = dictionaries();
    let map = if traditional { t } else { s };
    text.chars().any(|c| map.contains_key(&c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversion_preserves_unmapped_unicode() {
        assert_eq!(convert("简体漫画 😀 abc", false), "簡體漫畫 😀 abc");
        assert_eq!(convert("繁體漫畫 😀 abc", true), "繁体漫画 😀 abc");
        assert!(!contains("abc😀", false));
        assert!(contains("漫画", false));
        assert!(contains("漫畫", true));
    }
}

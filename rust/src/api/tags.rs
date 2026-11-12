use std::sync::OnceLock;

fn dictionary(traditional: bool) -> &'static serde_json::Value {
    static CN: OnceLock<serde_json::Value> = OnceLock::new();
    static TW: OnceLock<serde_json::Value> = OnceLock::new();
    let (cell, text) = if traditional {
        (&TW, include_str!("../../../assets/tags_tw.json"))
    } else {
        (&CN, include_str!("../../../assets/tags.json"))
    };
    cell.get_or_init(|| serde_json::from_str(text).expect("embedded tag dictionary"))
}

fn lookup(text: &str, traditional: bool) -> Option<&'static str> {
    let data = dictionary(traditional);
    [
        "male",
        "female",
        "language",
        "parody",
        "character",
        "other",
        "mixed",
    ]
    .iter()
    .find_map(|namespace| data[*namespace][text].as_str())
}

#[flutter_rust_bridge::frb(sync)]
pub fn translate_tag(text: String, namespace: Option<String>, traditional: bool) -> String {
    if let Some(namespace) = namespace {
        let lower = text.to_lowercase();
        return dictionary(traditional)[&namespace][&lower]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| {
                if dictionary(traditional)[&namespace].is_object() {
                    lower
                } else {
                    lookup(&lower, traditional).unwrap_or(&lower).to_owned()
                }
            });
    }
    if text.contains('|') {
        return text
            .split('|')
            .take(2)
            .find_map(|v| lookup(v.trim(), traditional))
            .unwrap_or(&text)
            .to_owned();
    }
    if let Some((namespace, tag)) = text.split_once(':') {
        if dictionary(traditional)[namespace].is_object() {
            return translate_tag(
                tag.split(':').next().unwrap_or(tag).into(),
                Some(namespace.into()),
                traditional,
            );
        }
        return text;
    }
    lookup(&text, traditional).unwrap_or(&text).to_owned()
}

pub struct TagSuggestion {
    pub text: String,
    pub namespace: String,
}

#[flutter_rust_bridge::frb(sync)]
pub fn tag_suggestions(text: String, traditional: bool, limit: u32) -> Vec<TagSuggestion> {
    let mut result = Vec::new();
    if text.trim().is_empty() {
        return result;
    }
    let limit = limit.min(500) as usize;
    for namespace in [
        "female",
        "male",
        "parody",
        "character",
        "other",
        "mixed",
        "language",
        "artist",
        "group",
        "cosplayer",
    ] {
        if let Some(entries) = dictionary(traditional)[namespace].as_object() {
            for (key, value) in entries {
                if result.len() >= limit {
                    return result;
                }
                if key.starts_with(&text)
                    || key
                        .rsplit(' ')
                        .next()
                        .is_some_and(|last| last.starts_with(&text))
                    || value.as_str().is_some_and(|v| v.contains(&text))
                {
                    result.push(TagSuggestion {
                        text: key.clone(),
                        namespace: namespace.into(),
                    });
                }
            }
        }
    }
    result
}

pub(crate) fn translated_tags(tags: &[String], traditional: bool) -> String {
    tags.iter()
        .filter_map(|tag| {
            let translated = translate_tag(tag.clone(), None, traditional);
            (translated != *tag).then_some(translated)
        })
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn embedded_dictionaries_and_suggestion_limits() {
        assert!(dictionary(false).is_object());
        assert!(dictionary(true).is_object());
        assert_eq!(
            translate_tag("unrecognized:value".into(), None, false),
            "unrecognized:value"
        );
        assert!(tag_suggestions("  ".into(), false, 100).is_empty());
        let suggestions = tag_suggestions("a".into(), false, 3);
        assert_eq!(suggestions.len(), 3);
        assert!(!translate_tag(
            suggestions[0].text.clone(),
            Some(suggestions[0].namespace.clone()),
            false
        )
        .is_empty());
    }
}

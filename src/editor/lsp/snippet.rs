//! Expand completion placeholders into a single plain-text edit. Interactive
//! tab stops, variables, and regex transformations are not part of the editor.
use std::collections::HashMap;

pub(super) fn plain_text(source: &str) -> Option<String> {
    if source.len() > 64 * 1024 {
        return None;
    }
    expand(source, &mut 0, false, 0, &mut HashMap::new())
}

fn expand(
    source: &str,
    offset: &mut usize,
    nested: bool,
    depth: usize,
    stops: &mut HashMap<u32, String>,
) -> Option<String> {
    if depth > 16 {
        return None;
    }
    let mut result = String::new();
    while let Some(ch) = source[*offset..].chars().next() {
        *offset += ch.len_utf8();
        match ch {
            '}' if nested => return Some(result),
            '\\' => {
                let next = source[*offset..].chars().next()?;
                if matches!(next, '$' | '}' | '\\') {
                    *offset += next.len_utf8();
                    result.push(next);
                } else {
                    result.push('\\');
                }
            }
            '$' => {
                let braced = source.as_bytes().get(*offset) == Some(&b'{');
                if braced {
                    *offset += 1;
                }
                let start = *offset;
                while source
                    .as_bytes()
                    .get(*offset)
                    .is_some_and(u8::is_ascii_digit)
                {
                    *offset += 1;
                }
                let number: u32 = source[start..*offset].parse().ok()?;
                let value = if !braced {
                    stops.get(&number).cloned().unwrap_or_default()
                } else {
                    let separator = *source.as_bytes().get(*offset)?;
                    *offset += 1;
                    match separator {
                        b'}' => stops.get(&number).cloned().unwrap_or_default(),
                        b':' => {
                            let default = expand(source, offset, true, depth + 1, stops)?;
                            stops.entry(number).or_insert(default).clone()
                        }
                        b'|' => {
                            let mut first = String::new();
                            let mut chosen = false;
                            loop {
                                let ch = source[*offset..].chars().next()?;
                                *offset += ch.len_utf8();
                                match ch {
                                    '|' if source.as_bytes().get(*offset) == Some(&b'}') => {
                                        *offset += 1;
                                        break;
                                    }
                                    ',' => chosen = true,
                                    '\\' => {
                                        let escaped = source[*offset..].chars().next()?;
                                        if !matches!(escaped, '\\' | ',' | '|') {
                                            return None;
                                        }
                                        *offset += escaped.len_utf8();
                                        if !chosen {
                                            first.push(escaped);
                                        }
                                    }
                                    _ if !chosen => first.push(ch),
                                    _ => {}
                                }
                            }
                            stops.entry(number).or_insert(first).clone()
                        }
                        _ => return None,
                    }
                };
                result.push_str(&value);
            }
            _ => result.push(ch),
        }
        if result.len() > 64 * 1024 {
            return None;
        }
    }
    (!nested).then_some(result)
}

pub(super) fn normalize(item: &mut lsp_types::CompletionItem) -> bool {
    if item.insert_text_format != Some(lsp_types::InsertTextFormat::SNIPPET) {
        return true;
    }
    let text = match &mut item.text_edit {
        Some(lsp_types::CompletionTextEdit::Edit(edit)) => &mut edit.new_text,
        Some(lsp_types::CompletionTextEdit::InsertAndReplace(edit)) => &mut edit.new_text,
        None => item.insert_text.get_or_insert_with(|| item.label.clone()),
    };
    let Some(expanded) = plain_text(text) else {
        return false;
    };
    *text = expanded;
    item.insert_text_format = Some(lsp_types::InsertTextFormat::PLAIN_TEXT);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn expands_defaults_choices_mirrors_unicode_and_escapes() {
        assert_eq!(
            plain_text("<${1:button}>${2:日本語}</$1>$0"),
            Some("<button>日本語</button>".into())
        );
        assert_eq!(
            plain_text("${1:call(${2:value})}, $2, ${1}"),
            Some("call(value), value, call(value)".into())
        );
        assert_eq!(
            plain_text(r"${1|one\,two,three|} \$5 \\ ${2}"),
            Some("one,two $5 \\ ".into())
        );
        for source in [
            "${1:unfinished",
            "$TM_FILENAME",
            "${1/regex/replace/}",
            "${1|unfinished",
            "${x}",
        ] {
            assert!(plain_text(source).is_none(), "{source}");
        }
    }
    #[test]
    fn normalizes_each_completion_edit_shape_and_filters_unsupported_syntax() {
        for edit in [
            None,
            Some(lsp_types::CompletionTextEdit::Edit(lsp_types::TextEdit {
                range: Default::default(),
                new_text: "${1:button}$0".into(),
            })),
            Some(lsp_types::CompletionTextEdit::InsertAndReplace(
                lsp_types::InsertReplaceEdit {
                    new_text: "${1:button}$0".into(),
                    insert: Default::default(),
                    replace: Default::default(),
                },
            )),
        ] {
            let mut item = lsp_types::CompletionItem {
                label: "button".into(),
                insert_text: Some("${1:button}$0".into()),
                insert_text_format: Some(lsp_types::InsertTextFormat::SNIPPET),
                text_edit: edit,
                ..Default::default()
            };
            assert!(normalize(&mut item));
            assert_eq!(
                item.insert_text_format,
                Some(lsp_types::InsertTextFormat::PLAIN_TEXT)
            );
            let text = match item.text_edit {
                Some(lsp_types::CompletionTextEdit::Edit(edit)) => edit.new_text,
                Some(lsp_types::CompletionTextEdit::InsertAndReplace(edit)) => edit.new_text,
                None => item.insert_text.unwrap(),
            };
            assert_eq!(text, "button");
        }
    }
}

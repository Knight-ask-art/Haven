//! 共享 CJK bigram 分词器（前端 book-search.ts 与后端 reader_search 同源）。
//!
//! 为 FTS5 shadow 列与Bm25 排序提供一致的 tokenization，避免前后端漂移。

/// 全角 → 半角（U+FF01..FF5E → 0x21..0x7E，U+3000 → 空格）。
pub fn full_width_to_half_width(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    for ch in text.chars() {
        let code = ch as u32;
        if (0xFF01..=0xFF5E).contains(&code) {
            result.push(char::from_u32(code - 0xFEE0).unwrap_or(ch));
        } else if code == 0x3000 {
            result.push(' ');
        } else {
            result.push(ch);
        }
    }
    result
}

fn is_whitespace_code(code: u32) -> bool {
    matches!(code, 0x20 | 0x09 | 0x0A | 0x0D | 0x0C | 0x3000)
}

pub fn normalize(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut pending_space = false;
    for ch in full_width_to_half_width(text).chars() {
        let code = ch as u32;
        if is_whitespace_code(code) {
            pending_space = true;
            continue;
        }
        if pending_space && !result.is_empty() {
            result.push(' ');
        }
        pending_space = false;
        for lower in ch.to_lowercase() {
            result.push(lower);
        }
    }
    result
}

/// 排名用分词：CJK 连续串按 2-gram，ASCII 按单词，其余丢弃。
/// 一字查询额外保留单字（与前端 book-search.ts 1:1）。
pub fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cjk_run = String::new();
    let mut word_run = String::new();
    let flush_word = |word_run: &mut String, tokens: &mut Vec<String>| {
        if !word_run.is_empty() {
            tokens.push(word_run.to_lowercase());
            word_run.clear();
        }
    };
    let flush_cjk = |cjk_run: &mut String, tokens: &mut Vec<String>| {
        if cjk_run.is_empty() {
            return;
        }
        if cjk_run.chars().count() == 1 {
            tokens.push(cjk_run.clone());
        } else {
            let chars: Vec<char> = cjk_run.chars().collect();
            for i in 0..chars.len() - 1 {
                tokens.push(chars[i..i + 2].iter().collect());
            }
        }
        cjk_run.clear();
    };
    for ch in full_width_to_half_width(text).chars() {
        let code = ch as u32;
        if (0x4E00..=0x9FFF).contains(&code) || (0x3400..=0x4DBF).contains(&code) {
            flush_word(&mut word_run, &mut tokens);
            cjk_run.push(ch);
        } else if ch.is_ascii_alphanumeric() {
            flush_cjk(&mut cjk_run, &mut tokens);
            word_run.push(ch);
        } else {
            flush_cjk(&mut cjk_run, &mut tokens);
            flush_word(&mut word_run, &mut tokens);
        }
    }
    flush_cjk(&mut cjk_run, &mut tokens);
    flush_word(&mut word_run, &mut tokens);
    tokens
}

pub fn quote_fts_token(token: &str) -> String {
    format!("\"{}\"", token.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_full_width() {
        assert_eq!(normalize("ＡＢＣ　栖阅"), "abc 栖阅");
    }

    #[test]
    fn tokenize_cjk_bigram() {
        assert_eq!(tokenize("三体问题"), vec!["三体", "体问", "问题"]);
        assert_eq!(tokenize("三"), vec!["三"]);
    }

    #[test]
    fn tokenize_mixed() {
        assert_eq!(
            tokenize("the three body problem"),
            vec!["the", "three", "body", "problem"]
        );
    }
}

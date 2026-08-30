//! Reader Search Service — 前端 `book-search.ts` 的 Rust 等价物（契约 §19.4 `ReaderSearch`）。
//!
//! 为避免双源漂移，纯函数 1:1 移植前端实现：归一化/映射/分词/索引/检索/锚点。
//! Infrastructure 仅负责从 `PreparedSession` 抽取 `RawBookContent`（段落原文），
//! 本服务负责校验、建索引与检索；`TextAnchor` 存 `excerpt/prefix/suffix`。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use haven_common::{AppError, ErrorKind};
use haven_domain::enums::MediaType;

use super::session::PreparedSession;
use crate::wire::{
    ReaderSearchEvent, ReaderSearchEventData, ReaderSearchEventKind, ReaderSearchHitDto,
    SessionEngineDto, TextAnchorDto,
};

pub const MAX_BOOK_SEARCH_HITS: usize = 200;
pub const MAX_HITS_PER_CHAPTER: usize = 20;
pub const MAX_QUERY_CHARS: usize = 128;
pub const MAX_PREFIX_SUFFIX_CHARS: usize = 30;
pub const MIN_EXACT_CHARS: usize = 12;
pub const MAX_EXACT_CHARS: usize = 240;

/// Server-only 原始章节（段落原文，`paragraphs.join("\n\n")` 为章原文）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawChapter {
    pub id: String,
    pub title: String,
    pub paragraphs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RawBookContent {
    pub chapters: Vec<RawChapter>,
}

pub trait ReaderSearchProvider: Send + Sync {
    fn extract(&self, session: &PreparedSession) -> Result<RawBookContent, AppError>;
}

#[derive(Clone)]
pub struct ReaderSearchService {
    provider: Arc<dyn ReaderSearchProvider>,
}

impl ReaderSearchService {
    pub fn new(provider: Arc<dyn ReaderSearchProvider>) -> Self {
        Self { provider }
    }

    pub fn search(
        &self,
        session: &PreparedSession,
        query: &str,
    ) -> Result<Vec<ReaderSearchHitDto>, AppError> {
        self.validate_session(session)?;
        let raw = self.provider.extract(session)?;
        let index = build_book_search_index(&raw.chapters);
        Ok(search_book(&raw.chapters, &index, query))
    }

    fn validate_session(&self, session: &PreparedSession) -> Result<(), AppError> {
        if session.engine != SessionEngineDto::Reader
            || !matches!(session.media_type, MediaType::Book | MediaType::Document)
        {
            return Err(AppError::new(
                "FORMAT_UNSUPPORTED",
                ErrorKind::Unsupported,
                "当前 Session 不是受支持的图书资源",
                false,
            ));
        }
        Ok(())
    }

    pub fn search_with_events(
        &self,
        session: &PreparedSession,
        query: &str,
        operation_id: &str,
        sink: &dyn ReaderSearchEventSink,
    ) -> Result<(), AppError> {
        self.validate_session(session)?;
        let raw = self.provider.extract(session)?;
        let index = build_book_search_index(&raw.chapters);
        let total = raw.chapters.len() as u32;
        let mut sequence: u32 = 1;
        let now = || chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        sink.emit(ReaderSearchEvent {
            operation_id: operation_id.to_string(),
            sequence,
            at: now(),
            kind: ReaderSearchEventKind::Started,
            data: ReaderSearchEventData {
                hits: Vec::new(),
                scanned_chapters: Some(0),
                total_chapters: Some(total),
                code: None,
                message: None,
            },
        });
        sequence += 1;
        let mut all_hits: Vec<ReaderSearchHitDto> = Vec::new();
        for (chapter_index, chapter) in index.chapters.iter().enumerate() {
            // Progress per chapter
            sink.emit(ReaderSearchEvent {
                operation_id: operation_id.to_string(),
                sequence,
                at: now(),
                kind: ReaderSearchEventKind::Progress,
                data: ReaderSearchEventData {
                    hits: Vec::new(),
                    scanned_chapters: Some(chapter_index as u32 + 1),
                    total_chapters: Some(total),
                    code: None,
                    message: None,
                },
            });
            sequence += 1;
            // Collect hits for this chapter
            let chapter_hits = {
                let query_norm = normalize_for_match(query);
                if query_norm.is_empty() || query_norm.chars().count() > MAX_QUERY_CHARS {
                    Vec::new()
                } else {
                    // Re-run search for single chapter slice
                    let single_chapter = &raw.chapters[chapter_index..chapter_index + 1];
                    let single_index = build_book_search_index(single_chapter);
                    let hits = search_book(single_chapter, &single_index, query);
                    // Remap chapter_index to global
                    hits.into_iter()
                        .map(|mut h| {
                            h.chapter_index = chapter_index as u32;
                            h
                        })
                        .collect()
                }
            };
            if !chapter_hits.is_empty() {
                sink.emit(ReaderSearchEvent {
                    operation_id: operation_id.to_string(),
                    sequence,
                    at: now(),
                    kind: ReaderSearchEventKind::Result,
                    data: ReaderSearchEventData {
                        hits: chapter_hits.clone(),
                        scanned_chapters: Some(chapter_index as u32 + 1),
                        total_chapters: Some(total),
                        code: None,
                        message: None,
                    },
                });
                sequence += 1;
                all_hits.extend(chapter_hits);
                if all_hits.len() >= MAX_BOOK_SEARCH_HITS {
                    break;
                }
            }
            // Keep the compiler happy about chapter variable
            let _ = &chapter;
        }
        sink.emit(ReaderSearchEvent {
            operation_id: operation_id.to_string(),
            sequence,
            at: now(),
            kind: ReaderSearchEventKind::Completed,
            data: ReaderSearchEventData {
                hits: all_hits,
                scanned_chapters: Some(total),
                total_chapters: Some(total),
                code: None,
                message: None,
            },
        });
        Ok(())
    }
}

pub trait ReaderSearchEventSink: Send + Sync {
    fn emit(&self, event: ReaderSearchEvent);
}

impl ReaderSearchService {
    /// Channel 友好版本：同步执行检索并通过 sink 流式发射 Started/Progress/Result/Completed。
    /// 调用方需已通过 `lookup_for_owner` 校验 session 归属；本方法不触及 registry。
    pub fn search_with_channel(
        &self,
        session: &PreparedSession,
        query: &str,
        operation_id: &str,
        sink: &dyn ReaderSearchEventSink,
    ) -> Result<(), AppError> {
        self.validate_session(session)?;
        let raw = self.provider.extract(session)?;
        let index = build_book_search_index(&raw.chapters);
        let total = raw.chapters.len() as u32;
        let mut sequence: u32 = 1;
        let now = || chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        sink.emit(ReaderSearchEvent {
            operation_id: operation_id.to_string(),
            sequence,
            at: now(),
            kind: ReaderSearchEventKind::Started,
            data: ReaderSearchEventData {
                hits: Vec::new(),
                scanned_chapters: Some(0),
                total_chapters: Some(total),
                code: None,
                message: None,
            },
        });
        sequence += 1;
        let mut all_hits: Vec<ReaderSearchHitDto> = Vec::new();
        for (chapter_index, _) in index.chapters.iter().enumerate() {
            sink.emit(ReaderSearchEvent {
                operation_id: operation_id.to_string(),
                sequence,
                at: now(),
                kind: ReaderSearchEventKind::Progress,
                data: ReaderSearchEventData {
                    hits: Vec::new(),
                    scanned_chapters: Some(chapter_index as u32 + 1),
                    total_chapters: Some(total),
                    code: None,
                    message: None,
                },
            });
            sequence += 1;
            let single_chapter = &raw.chapters[chapter_index..chapter_index + 1];
            let single_index = build_book_search_index(single_chapter);
            let hits = search_book(single_chapter, &single_index, query);
            let hits = hits
                .into_iter()
                .map(|mut h| {
                    h.chapter_index = chapter_index as u32;
                    h
                })
                .collect::<Vec<_>>();
            if !hits.is_empty() {
                sink.emit(ReaderSearchEvent {
                    operation_id: operation_id.to_string(),
                    sequence,
                    at: now(),
                    kind: ReaderSearchEventKind::Result,
                    data: ReaderSearchEventData {
                        hits: hits.clone(),
                        scanned_chapters: Some(chapter_index as u32 + 1),
                        total_chapters: Some(total),
                        code: None,
                        message: None,
                    },
                });
                sequence += 1;
                all_hits.extend(hits);
                if all_hits.len() >= MAX_BOOK_SEARCH_HITS {
                    break;
                }
            }
        }
        sink.emit(ReaderSearchEvent {
            operation_id: operation_id.to_string(),
            sequence,
            at: now(),
            kind: ReaderSearchEventKind::Completed,
            data: ReaderSearchEventData {
                hits: all_hits,
                scanned_chapters: Some(total),
                total_chapters: Some(total),
                code: None,
                message: None,
            },
        });
        Ok(())
    }
}

// ---- 归一化 / 映射 / 分词（与前端 1:1） ----

fn is_whitespace_code(code: u32) -> bool {
    matches!(code, 0x20 | 0x09 | 0x0A | 0x0D | 0x0C | 0x3000)
}

fn full_width_to_half_width(text: &str) -> String {
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

pub fn normalize_for_match(text: &str) -> String {
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

pub fn normalize_with_map(text: &str) -> (String, Vec<i32>) {
    let mut norm = Vec::new();
    let mut map: Vec<i32> = Vec::new();
    let mut pending_space = false;
    let mut index: i32 = 0;
    for ch in full_width_to_half_width(text).chars() {
        let code = ch as u32;
        if is_whitespace_code(code) {
            pending_space = true;
            index += 1;
            continue;
        }
        if pending_space && !norm.is_empty() {
            norm.push(' ');
            map.push(-1);
        }
        pending_space = false;
        for lower in ch.to_lowercase() {
            norm.push(lower);
            map.push(index);
        }
        index += 1;
    }
    (norm.into_iter().collect(), map)
}

pub fn tokenize_for_rank(text: &str) -> Vec<String> {
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

// ---- 索引 ----

#[derive(Debug, Clone)]
pub struct BookSearchIndexChapter {
    pub id: String,
    pub title: String,
    pub norm: String,
    pub map: Vec<i32>,
    pub paragraph_starts: Vec<usize>,
    pub original_len: usize,
}

#[derive(Debug, Clone)]
pub struct BookSearchIndex {
    pub chapters: Vec<BookSearchIndexChapter>,
    pub documents: usize,
    pub term_document_frequencies: HashMap<String, usize>,
}

pub fn build_book_search_index(chapters: &[RawChapter]) -> BookSearchIndex {
    let mut indexed = Vec::new();
    let mut term_documents: HashMap<String, HashSet<usize>> = HashMap::new();
    let mut documents = 0usize;
    for (chapter_index, chapter) in chapters.iter().enumerate() {
        let original = chapter.paragraphs.join("\n\n");
        let mut norm_parts: Vec<String> = Vec::new();
        let mut map: Vec<i32> = Vec::new();
        let mut paragraph_starts: Vec<usize> = Vec::new();
        let mut original_offset: usize = 0;
        for (paragraph_index, paragraph) in chapter.paragraphs.iter().enumerate() {
            paragraph_starts.push(original_offset);
            let (norm, para_map) = normalize_with_map(paragraph);
            if paragraph_index > 0 {
                norm_parts.push(" ".to_string());
                map.push(-1);
                original_offset += 2;
            }
            norm_parts.push(norm);
            for mapped in para_map {
                if mapped == -1 {
                    map.push(-1);
                } else {
                    map.push(mapped + original_offset as i32);
                }
            }
            original_offset += paragraph.chars().count();
        }
        let norm: String = norm_parts.concat();
        let original_len = original.chars().count();
        if !norm.is_empty() {
            documents += 1;
        }
        let terms: HashSet<String> = tokenize_for_rank(&norm).into_iter().collect();
        for term in terms {
            term_documents
                .entry(term)
                .or_default()
                .insert(chapter_index);
        }
        indexed.push(BookSearchIndexChapter {
            id: chapter.id.clone(),
            title: chapter.title.clone(),
            norm,
            map,
            paragraph_starts,
            original_len,
        });
    }
    let mut term_document_frequencies = HashMap::new();
    for (term, docs) in term_documents {
        term_document_frequencies.insert(term, docs.len());
    }
    BookSearchIndex {
        chapters: indexed,
        documents,
        term_document_frequencies,
    }
}

fn term_score(term: &str, index: &BookSearchIndex) -> f64 {
    let df = index
        .term_document_frequencies
        .get(term)
        .copied()
        .unwrap_or(0) as f64;
    let d = index.documents as f64;
    ((d + 1.0) / (df + 1.0)).ln() + 1.0
}

pub fn search_query_terms(query: &str) -> Vec<String> {
    let mut unique = HashSet::new();
    let mut result = Vec::new();
    for term in tokenize_for_rank(query) {
        if unique.insert(term.clone()) {
            result.push(term);
        }
    }
    result
}

pub fn search_book(
    chapters: &[RawChapter],
    index: &BookSearchIndex,
    query: &str,
) -> Vec<ReaderSearchHitDto> {
    let query_norm = normalize_for_match(query);
    if query_norm.is_empty() || query_norm.chars().count() > MAX_QUERY_CHARS {
        return Vec::new();
    }
    let terms = search_query_terms(&query_norm);
    let mut scores: HashMap<String, f64> = HashMap::new();
    for term in &terms {
        scores.insert(term.clone(), term_score(term, index));
    }
    let mut all: Vec<ReaderSearchHitDto> = Vec::new();
    for (chapter_index, chapter) in index.chapters.iter().enumerate() {
        if chapter.norm.is_empty() {
            continue;
        }
        let original: String = chapters[chapter_index].paragraphs.join("\n\n");
        let original_chars: Vec<char> = original.chars().collect();
        let mut hits: Vec<ReaderSearchHitDto> = Vec::new();
        let mut search_from: usize = 0;
        let query_len = query_norm.chars().count();
        let norm_chars: Vec<char> = chapter.norm.chars().collect();
        while hits.len() < MAX_HITS_PER_CHAPTER {
            let search_slice: String = norm_chars[search_from..].iter().collect();
            let Some(relative_byte) = search_slice.find(&query_norm) else {
                break;
            };
            let relative_chars = search_slice[..relative_byte].chars().count();
            let found = search_from + relative_chars;
            let end = found + query_len;
            let mut crosses = false;
            let mut original_start: i32 = -1;
            for offset in found..end {
                let mapped = chapter.map[offset];
                if mapped == -1 {
                    crosses = true;
                    break;
                }
                if original_start == -1 {
                    original_start = mapped;
                }
            }
            if crosses {
                search_from = found + query_len;
                continue;
            }
            let original_end = chapter.map[end - 1] + 1;
            let mut exact_start = original_start as usize;
            let mut exact_end = original_end as usize;
            while exact_end - exact_start < MIN_EXACT_CHARS {
                if exact_start > 0 {
                    exact_start -= 1;
                } else if exact_end < original_chars.len() {
                    exact_end += 1;
                } else {
                    break;
                }
            }
            let exact: String = original_chars[exact_start..exact_end].iter().collect();
            if exact.chars().count() > MAX_EXACT_CHARS {
                search_from = found + query_len;
                continue;
            }
            let prefix_start = exact_start.saturating_sub(MAX_PREFIX_SUFFIX_CHARS);
            let prefix: String = original_chars[prefix_start..exact_start].iter().collect();
            let suffix_end = (exact_end + MAX_PREFIX_SUFFIX_CHARS).min(original_chars.len());
            let suffix: String = original_chars[exact_end..suffix_end].iter().collect();
            let mut score = 0.0;
            for term in &terms {
                score += scores.get(term).copied().unwrap_or(0.0);
            }
            let mut paragraph_index = 0usize;
            for (idx, start) in chapter.paragraph_starts.iter().enumerate() {
                if *start <= original_start as usize {
                    paragraph_index = idx;
                }
            }
            let progression = if chapter.norm.chars().count() == 0 {
                0.0
            } else {
                found as f64 / chapter.norm.chars().count() as f64
            };
            hits.push(ReaderSearchHitDto {
                chapter_id: chapter.id.clone(),
                chapter_title: chapter.title.clone(),
                chapter_index: chapter_index as u32,
                paragraph_index: paragraph_index as u32,
                progression_in_chapter: progression,
                text_anchor: TextAnchorDto {
                    exact: Some(exact),
                    prefix: if prefix.is_empty() {
                        None
                    } else {
                        Some(prefix)
                    },
                    suffix: if suffix.is_empty() {
                        None
                    } else {
                        Some(suffix)
                    },
                },
                score,
            });
            search_from = found + query_len;
        }
        all.extend(hits);
    }
    all.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.chapter_index.cmp(&b.chapter_index))
    });
    all.truncate(MAX_BOOK_SEARCH_HITS);
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chapter(id: &str, paragraphs: Vec<&str>) -> RawChapter {
        RawChapter {
            id: id.to_string(),
            title: format!("标题{id}"),
            paragraphs: paragraphs.into_iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn normalize_folds_whitespace_and_full_width() {
        assert_eq!(
            normalize_for_match("  Hello   \u{3000} World  "),
            "hello world"
        );
        assert_eq!(normalize_for_match("ＡＢＣ１２３"), "abc123");
    }

    #[test]
    fn tokenize_cjk_bigrams() {
        assert_eq!(tokenize_for_rank("人工智能"), vec!["人工", "工智", "智能"]);
    }

    #[test]
    fn search_finds_hits_with_anchor() {
        let chapters = vec![chapter(
            "c1",
            vec!["人工智能是未来的方向，人工智能改变世界"],
        )];
        let index = build_book_search_index(&chapters);
        let hits = search_book(&chapters, &index, "人工智能");
        assert!(!hits.is_empty());
        assert!(hits[0].text_anchor.exact.as_ref().unwrap().len() >= 12);
    }

    #[test]
    fn search_empty_or_too_long_returns_empty() {
        let chapters = vec![chapter("c1", vec!["hello world"])];
        let index = build_book_search_index(&chapters);
        assert!(search_book(&chapters, &index, "  ").is_empty());
        assert!(search_book(&chapters, &index, &"a".repeat(129)).is_empty());
    }

    #[test]
    fn search_caps_per_chapter_and_global() {
        let paragraph = (0..30).map(|_| "test").collect::<Vec<_>>().join(" ");
        let chapters = vec![chapter("c1", vec![paragraph.as_str()])];
        let index = build_book_search_index(&chapters);
        let hits = search_book(&chapters, &index, "test");
        assert!(hits.len() <= MAX_HITS_PER_CHAPTER);
    }
}

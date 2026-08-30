//! 豆瓣热榜 Provider。
//!
//! Provider 只返回 Application 内部的 candidate，不生产 Wire DTO，也不负责
//! 缓存和 Artwork 登记。真实组合根不再使用静态榜单兜底；静态实现仅供测试/Browser Mock。

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use haven_application::services::trending::{
    RemoteArtworkCandidate, TrendingBoardCandidate, TrendingItemCandidate, TrendingProvider,
};
use haven_common::AppError;
use tokio::sync::Semaphore;

pub struct StaticTrendingProvider;

impl Default for StaticTrendingProvider {
    fn default() -> Self {
        Self
    }
}

#[async_trait]
impl TrendingProvider for StaticTrendingProvider {
    async fn fetch_boards(
        &self,
        board_ids: &[String],
    ) -> Result<Vec<TrendingBoardCandidate>, AppError> {
        Ok(board_ids
            .iter()
            .filter_map(|board_id| static_board(board_id))
            .collect())
    }
}

fn static_board(board_id: &str) -> Option<TrendingBoardCandidate> {
    let (title, item_title, subtitle, description, status_badge) = match board_id {
        "anime" => (
            "动漫热门",
            "海贼王",
            "2026 / 日本 / 冒险",
            "草帽海贼团的伟大航程，追寻 One Piece 的梦想。",
            None,
        ),
        "cn_drama" => (
            "国产剧热门",
            "庆余年 第二季",
            "2024 / 中国 / 古装",
            "范闲归来再掀风云。",
            Some("更新至12集"),
        ),
        "variety" => (
            "综艺热门",
            "乘风 2026",
            "2026 / 中国 / 真人秀",
            "国际女性文化交流与音乐竞演。",
            None,
        ),
        "us_drama" => (
            "英美剧热门",
            "黑袍纠察队 第五季",
            "2026 / 美国 / 科幻",
            "祖国人的世界迎来终章。",
            None,
        ),
        _ => return None,
    };
    Some(TrendingBoardCandidate {
        board_id: board_id.to_owned(),
        title: title.to_owned(),
        subtitle: "TOP 10".into(),
        items: vec![TrendingItemCandidate {
            title: item_title.into(),
            subtitle: subtitle.into(),
            description: description.into(),
            poster: None,
            status_badge: status_badge.map(str::to_owned),
        }],
    })
}

const DOUBAN_COLLECTIONS: &[(&str, &str, &str)] = &[
    (
        "anime",
        "动漫热门",
        "https://m.douban.com/rexxar/api/v2/subject_collection/tv_animation/items?items_only=1&start=0&count=10&for_mobile=1",
    ),
    (
        "cn_drama",
        "国产剧热门",
        "https://m.douban.com/rexxar/api/v2/subject_collection/tv_domestic/items?items_only=1&start=0&count=10&for_mobile=1",
    ),
    (
        "variety",
        "综艺热门",
        "https://m.douban.com/rexxar/api/v2/subject_collection/tv_variety_show/items?items_only=1&start=0&count=10&for_mobile=1",
    ),
    (
        "us_drama",
        "英美剧热门",
        "https://m.douban.com/rexxar/api/v2/subject_collection/tv_american/items?items_only=1&start=0&count=10&for_mobile=1",
    ),
];

/// 豆瓣 rexxar 4 榜网络实现：最多两个榜单并发，每榜连接 1.5s、总请求 3s。
pub struct DoubanTrendingProvider {
    client: reqwest::Client,
}

impl DoubanTrendingProvider {
    pub fn new() -> Result<Self, AppError> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_millis(1_500))
            .timeout(Duration::from_secs(3))
            .user_agent("Mozilla/5.0 (Linux; Android 13; Pixel 7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Mobile Safari/537.36")
            .build()
            .map_err(|_| {
                AppError::new(
                    "TRENDING_PROVIDER_INIT_FAILED",
                    haven_common::ErrorKind::Internal,
                    "热榜来源初始化失败",
                    false,
                )
            })?;
        Ok(Self { client })
    }

    async fn fetch_one(
        client: reqwest::Client,
        board_id: String,
        title: String,
        url: String,
    ) -> Option<TrendingBoardCandidate> {
        let resp = client
            .get(url)
            .header("Referer", "https://m.douban.com/")
            .header("Accept", "application/json")
            .send()
            .await
            .ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let json: serde_json::Value = resp.json().await.ok()?;
        let items = json
            .get("subject_collection_items")
            .and_then(|v| v.as_array())?;
        let mut out_items = Vec::new();
        for raw in items.iter().take(10) {
            let title = raw.get("title").and_then(|v| v.as_str()).unwrap_or("");
            if title.trim().is_empty() {
                continue;
            }
            let subtitle = raw
                .get("card_subtitle")
                .and_then(|v| v.as_str())
                .or_else(|| raw.get("info").and_then(|v| v.as_str()))
                .unwrap_or("")
                .to_owned();
            let description = raw
                .get("comment")
                .and_then(|v| v.as_str())
                .or_else(|| raw.get("description").and_then(|v| v.as_str()))
                .unwrap_or("")
                .chars()
                .take(80)
                .collect::<String>();
            let poster = first_cover_url(raw).and_then(|target_url| {
                is_douban_artwork_url(&target_url).then_some(RemoteArtworkCandidate {
                    source_id: "douban".into(),
                    target_url,
                })
            });
            let status_badge = raw
                .get("rank")
                .and_then(|v| v.as_u64())
                .map(|rank| format!("TOP {rank}"))
                .or_else(|| {
                    raw.get("rating")
                        .and_then(|v| v.get("value"))
                        .and_then(|v| v.as_f64())
                        .map(|rating| format!("{rating:.1}分"))
                });
            out_items.push(TrendingItemCandidate {
                title: title.to_owned(),
                subtitle,
                description,
                poster,
                status_badge,
            });
        }
        (!out_items.is_empty()).then_some(TrendingBoardCandidate {
            board_id,
            title,
            subtitle: "TOP 10".into(),
            items: out_items,
        })
    }
}

#[async_trait]
impl TrendingProvider for DoubanTrendingProvider {
    async fn fetch_boards(
        &self,
        board_ids: &[String],
    ) -> Result<Vec<TrendingBoardCandidate>, AppError> {
        let semaphore = Arc::new(Semaphore::new(2));
        let mut handles = Vec::new();
        for board_id in board_ids {
            let Some((_, title, url)) = DOUBAN_COLLECTIONS.iter().find(|(id, _, _)| id == board_id)
            else {
                continue;
            };
            let permit = semaphore.clone().acquire_owned().await.map_err(|_| {
                AppError::new(
                    "TRENDING_PROVIDER_CANCELLED",
                    haven_common::ErrorKind::Cancelled,
                    "热榜刷新已取消",
                    true,
                )
            })?;
            let client = self.client.clone();
            let board_id = board_id.clone();
            let title = (*title).to_owned();
            let url = (*url).to_owned();
            handles.push(tokio::spawn(async move {
                let _permit = permit;
                Self::fetch_one(client, board_id, title, url).await
            }));
        }

        let mut boards = Vec::new();
        for handle in handles {
            if let Ok(Some(board)) = handle.await {
                boards.push(board);
            }
        }
        boards.sort_by_key(|board| {
            DOUBAN_COLLECTIONS
                .iter()
                .position(|(id, _, _)| id == &board.board_id)
                .unwrap_or(usize::MAX)
        });
        Ok(boards)
    }
}

fn first_cover_url(raw: &serde_json::Value) -> Option<String> {
    raw.get("pic")
        .and_then(|v| v.get("large"))
        .and_then(|v| v.as_str())
        .or_else(|| {
            raw.get("pic")
                .and_then(|v| v.get("normal"))
                .and_then(|v| v.as_str())
        })
        .or_else(|| {
            raw.get("cover")
                .and_then(|v| v.get("url"))
                .and_then(|v| v.as_str())
        })
        .or_else(|| raw.get("cover_url").and_then(|v| v.as_str()))
        .or_else(|| {
            raw.get("photos")
                .and_then(|v| v.as_array())
                .and_then(|array| array.first())
                .and_then(|v| v.as_str())
        })
        .map(str::to_owned)
}

fn is_douban_artwork_url(value: &str) -> bool {
    let Ok(url) = value.parse::<reqwest::Url>() else {
        return false;
    };
    if url.scheme() != "https" || url.username() != "" || url.password().is_some() {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    host == "doubanio.com" || host.ends_with(".doubanio.com")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artwork_host_policy_is_exact() {
        assert!(is_douban_artwork_url(
            "https://img.doubanio.com/view/photo/l/public/p.jpg"
        ));
        assert!(is_douban_artwork_url("https://doubanio.com/p.jpg"));
        assert!(!is_douban_artwork_url("https://evil-doubanio.com/p.jpg"));
        assert!(!is_douban_artwork_url("http://img.doubanio.com/p.jpg"));
        assert!(!is_douban_artwork_url(
            "https://img.doubanio.com@evil.example/p.jpg"
        ));
    }

    #[tokio::test]
    async fn static_provider_only_returns_requested_boards() {
        let provider = StaticTrendingProvider;
        let boards = provider
            .fetch_boards(&["anime".into(), "missing".into()])
            .await
            .unwrap();
        assert_eq!(boards.len(), 1);
        assert_eq!(boards[0].board_id, "anime");
    }
}

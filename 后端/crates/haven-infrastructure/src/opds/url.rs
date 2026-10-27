//! OPDS 地址构造的唯一所有者。
//!
//! 搜索模板、OpenSearch 描述文件、feed 链接、条目链接与回退查询串全部经过这里，
//! 因此「相对引用怎么绝对化」与「哪些主机属于 Gutenberg 官方镜像」只有一份判定。
//!
//! 契约：
//! - **保留 base 的 scheme 与端口**。这里曾经无条件把 `http` 升级成 `https`（绝对值、
//!   协议相对值都升），并且只接受以 `/` 开头的相对引用。对内置 Gutenberg 那套写法是
//!   必要的（该站明文会断连），但对用户登记的自托管目录是错的：匿名 HTTP 目录能保存、
//!   也能取到根目录，却会因为搜索地址与条目链接被改写成 `https` 而落到另一个 origin，
//!   在来源护栏前全部解析失败。
//! - 相对引用按 RFC 3986 与 base 做标准 join（绝对地址、协议相对、根相对、路径相对），
//!   不再只接受以 `/` 开头的形态。
//! - 只有 base 本身是 Gutenberg 官方镜像时，才把 `http` 结果按 `https` 取；这条窄
//!   行为只服务于那两个公开镜像，不对其它来源生效。

fn is_gutenberg_mirror_host(url: &reqwest::Url) -> bool {
    url.host_str()
        .map(str::to_ascii_lowercase)
        .is_some_and(|host| matches!(host.as_str(), "www.gutenberg.org" | "m.gutenberg.org"))
}

/// 相对引用绝对化：标准 base-URL join，保留 base 的 scheme 与端口（唯一例外见模块文档）。
pub(super) fn absolutize(base: &str, href: &str) -> Option<String> {
    let base = reqwest::Url::parse(base).ok()?;
    let joined = base.join(href.trim()).ok()?;
    if joined.scheme() == "http" && is_gutenberg_mirror_host(&base) {
        // Gutenberg 的 OpenSearch 模板与条目链接均为 http：该站的明文入口会断连，
        // 内置镜像统一按 https 取。自定义目录不走这一条。
        let mut upgraded = joined;
        upgraded.set_scheme("https").ok()?;
        return Some(upgraded.to_string());
    }
    Some(joined.to_string())
}

pub(super) fn is_gutenberg_endpoint(endpoint: &str) -> bool {
    reqwest::Url::parse(endpoint)
        .ok()
        .is_some_and(|url| is_gutenberg_mirror_host(&url))
}

/// Gutenberg 搜索镜像：www 优先（m. 在部分网络对 search.opds 返回 504）。
pub(super) fn gutenberg_search_mirrors(query: &str) -> Vec<String> {
    let q = urlencode(query);
    vec![
        format!("https://www.gutenberg.org/ebooks/search.opds/?query={q}"),
        format!("https://m.gutenberg.org/ebooks/search.opds/?query={q}"),
    ]
}

pub(super) fn with_gutenberg_host_fallbacks(url: &str) -> Vec<String> {
    let mut out = vec![url.to_owned()];
    if url.contains("://m.gutenberg.org") {
        out.push(url.replacen("://m.gutenberg.org", "://www.gutenberg.org", 1));
    } else if url.contains("://www.gutenberg.org") {
        out.push(url.replacen("://www.gutenberg.org", "://m.gutenberg.org", 1));
    }
    out
}

pub(super) fn urlencode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for b in input.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Return whether the raw URL authority contains an explicit port.  This is
/// intentionally only used after URL parsing and user-info rejection; it
/// exists because the URL parser erases the default HTTPS port when exposing
/// `Url::port()`.  Gutenberg only needs the bare public HTTPS hosts.
pub(super) fn has_explicit_authority_port(raw: &str) -> bool {
    let Some((_, after_scheme)) = raw.split_once("://") else {
        return false;
    };
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    if host.starts_with('[') {
        return host.find(']').is_some_and(|end| {
            host.get(end + 1..)
                .is_some_and(|tail| tail.starts_with(':'))
        });
    }
    host.contains(':')
}

#[cfg(test)]
mod tests {
    use super::super::extract_atom_template;
    use super::*;

    /// 匿名公开 HTTP 目录（无凭据）的地址构造必须原样保留 scheme 与端口：否则搜索
    /// 地址、OpenSearch 描述文件与条目链接都会落到另一个 origin，被来源护栏拒绝。
    #[test]
    fn absolutize_keeps_the_catalog_scheme_and_port() {
        let base = "http://catalog.example.invalid:8096/opds";
        assert_eq!(
            absolutize(base, "/books/9.opds").as_deref(),
            Some("http://catalog.example.invalid:8096/books/9.opds")
        );
        assert_eq!(
            absolutize(base, "http://catalog.example.invalid:8096/books/9.opds").as_deref(),
            Some("http://catalog.example.invalid:8096/books/9.opds")
        );
        // 协议相对引用继承 base 的 scheme（RFC 3986），而不是硬编码 https。
        assert_eq!(
            absolutize(base, "//catalog.example.invalid:8096/covers/9.png").as_deref(),
            Some("http://catalog.example.invalid:8096/covers/9.png")
        );
        // 路径相对引用同样按标准 join 解析。
        assert_eq!(
            absolutize(
                "http://catalog.example.invalid:8096/opds/v1/root",
                "../books/9.opds"
            )
            .as_deref(),
            Some("http://catalog.example.invalid:8096/opds/books/9.opds")
        );
        // 已登记的 HTTPS 目录不会被降级，也不会被升级成别的端口。
        assert_eq!(
            absolutize("https://catalog.example.invalid:8443/opds", "/books/9.opds").as_deref(),
            Some("https://catalog.example.invalid:8443/books/9.opds")
        );
        assert_eq!(
            absolutize("https://catalog.example.invalid/opds", "?query=x").as_deref(),
            Some("https://catalog.example.invalid/opds?query=x")
        );
        // 非 http(s) 的绝对引用原样保留，由调用方的策略判定拒绝。
        assert_eq!(
            absolutize(base, "ftp://catalog.example.invalid/9.epub").as_deref(),
            Some("ftp://catalog.example.invalid/9.epub")
        );
    }

    /// 只有内置 Gutenberg 镜像保留「明文结果按 https 取」这条窄行为。
    #[test]
    fn absolutize_keeps_the_narrow_gutenberg_https_upgrade() {
        let base = "https://m.gutenberg.org/ebooks/search.opds/?query=frankenstein";
        assert_eq!(
            absolutize(base, "http://m.gutenberg.org/ebooks/search.opds/?query=x").as_deref(),
            Some("https://m.gutenberg.org/ebooks/search.opds/?query=x")
        );
        assert_eq!(
            absolutize(base, "/cache/epub/84/pg84.epub").as_deref(),
            Some("https://m.gutenberg.org/cache/epub/84/pg84.epub")
        );
        assert_eq!(
            absolutize(base, "//cdn.example.org/a.epub").as_deref(),
            Some("https://cdn.example.org/a.epub")
        );
        assert_eq!(
            absolutize(base, "https://other.org/b.epub").as_deref(),
            Some("https://other.org/b.epub")
        );
        assert!(is_gutenberg_endpoint(
            "https://www.gutenberg.org/ebooks.opds/"
        ));
        assert!(!is_gutenberg_endpoint(
            "https://standardebooks.org/feeds/opds"
        ));
    }

    #[test]
    fn gutenberg_mirrors_prefer_www_then_mobile() {
        let urls = gutenberg_search_mirrors("frankenstein");
        assert_eq!(
            urls[0],
            "https://www.gutenberg.org/ebooks/search.opds/?query=frankenstein"
        );
        assert_eq!(
            urls[1],
            "https://m.gutenberg.org/ebooks/search.opds/?query=frankenstein"
        );
        assert_eq!(
            with_gutenberg_host_fallbacks("https://m.gutenberg.org/ebooks/84.opds"),
            vec![
                "https://m.gutenberg.org/ebooks/84.opds".to_owned(),
                "https://www.gutenberg.org/ebooks/84.opds".to_owned(),
            ]
        );
    }

    #[test]
    fn opensearch_picks_atom_template_and_upgrades_https() {
        let desc = r#"<?xml version="1.0"?>
<OpenSearchDescription xmlns="http://a9.com/-/spec/opensearch/1.1/">
  <ShortName>Gutenberg</ShortName>
  <Url type="text/html" template="http://www.gutenberg.org/ebooks/search/?query={searchTerms}"/>
  <Url type="application/atom+xml" template="http://m.gutenberg.org/ebooks/search.opds/?query={searchTerms}"/>
</OpenSearchDescription>"#;
        let tpl = extract_atom_template(desc).unwrap().template;
        let url = absolutize(
            "https://www.gutenberg.org/catalog/osd-books.xml",
            &tpl.replace("{searchTerms}", &urlencode("frankenstein")),
        )
        .unwrap();
        assert_eq!(
            url,
            "https://m.gutenberg.org/ebooks/search.opds/?query=frankenstein"
        );
    }

    #[test]
    fn explicit_authority_port_is_detected_from_the_raw_url() {
        assert!(has_explicit_authority_port(
            "https://www.gutenberg.org:443/ebooks.opds/"
        ));
        assert!(!has_explicit_authority_port(
            "https://www.gutenberg.org/ebooks.opds/"
        ));
        assert!(has_explicit_authority_port("https://[::1]:8443/a"));
        assert!(!has_explicit_authority_port("https://[::1]/a"));
    }
}

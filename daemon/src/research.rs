//! research.rs
//! -----------
//! When a request doesn't match anything in the memory bank, before asking
//! Jan.ai/DeepSeek to draft a plan, we do a couple of lightweight web
//! searches to ground the model in real, current package/unit names rather
//! than whatever it remembers from training. This text is passed as
//! read-only *context* into the prompt (see jan_client::research_plan) --
//! it is never itself parsed as commands. A malicious or wrong web page can
//! at worst produce a bad `reply` string or a step that fails
//! sanitize()/execution; it cannot inject an unconstrained action, because
//! the step vocabulary boundary in protogen-plan doesn't care where the
//! JSON came from.
use anyhow::Result;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(crate) struct SearchResult {
    title: String,
    snippet: String,
    url: String,
}

/// Uses DuckDuckGo's HTML-lite endpoint (no API key required) for a quick,
/// dependency-light web search. Swap for a proper search API key if you
/// want higher quality results.
pub async fn quick_search(query: &str, max_results: usize) -> Result<Vec<SearchResult>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;
    let resp = client
        .get("https://html.duckduckgo.com/html/")
        .query(&[("q", query)])
        .header("User-Agent", "Mozilla/5.0 (ProtogenOS assistant research)")
        .send()
        .await?;
    let body = resp.text().await?;
    Ok(parse_ddg_html(&body, max_results))
}

/// Minimal, dependency-free-ish HTML scrape of DDG's lite result markup.
/// Deliberately tolerant -- if the markup changes and this returns nothing,
/// research_context() below just falls back to no context and the model
/// relies on its own knowledge plus the known_apps/known_utilities lists.
fn parse_ddg_html(html: &str, max_results: usize) -> Vec<SearchResult> {
    let mut results = Vec::new();
    for block in html.split("result__body").skip(1).take(max_results) {
        let title = extract_between(block, "result__a\">", "</a>")
            .map(strip_tags)
            .unwrap_or_default();
        let snippet = extract_between(block, "result__snippet\">", "</a>")
            .map(strip_tags)
            .unwrap_or_default();
        let url = extract_between(block, "href=\"", "\"").unwrap_or_default().to_string();
        if !title.is_empty() {
            results.push(SearchResult { title, snippet, url });
        }
    }
    results
}

fn extract_between<'a>(haystack: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let s = haystack.find(start)? + start.len();
    let e = haystack[s..].find(end)? + s;
    Some(&haystack[s..e])
}

fn strip_tags(s: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.trim().to_string()
}

/// Builds the context block handed to the planner prompt: a couple of
/// targeted searches (package name lookup + "how to X on arch linux" style)
/// condensed into a few lines.
pub async fn research_context(user_request: &str, distro: &str) -> String {
    let queries = [
        format!("{user_request} {distro} package name"),
        format!("{user_request} arch wiki OR official docs"),
    ];
    let mut lines = Vec::new();
    for q in queries {
        match quick_search(&q, 3).await {
            Ok(results) => {
                for r in results {
                    lines.push(format!("- {} ({}): {}", r.title, r.url, r.snippet));
                }
            }
            Err(e) => {
                tracing::warn!("research search failed for '{q}': {e}");
            }
        }
    }
    if lines.is_empty() {
        String::new()
    } else {
        lines.join("\n")
    }
}

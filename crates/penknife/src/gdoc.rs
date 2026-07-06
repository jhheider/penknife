use anyhow::{Context, Result, bail};

/// Extract a Google Doc ID from a URL like
/// `https://docs.google.com/document/d/{ID}/edit`
pub fn extract_doc_id(url: &str) -> Option<String> {
    let marker = "/document/d/";
    let start = url.find(marker)? + marker.len();
    let rest = &url[start..];
    let end = rest.find('/').unwrap_or(rest.len());
    let id = &rest[..end];
    if id.is_empty() {
        None
    } else {
        Some(id.to_string())
    }
}

/// Fetch a Google Doc as markdown. The doc must be publicly accessible (link sharing on).
pub async fn fetch_doc_markdown(doc_id: &str) -> Result<String> {
    let url = format!("https://docs.google.com/document/d/{doc_id}/export?format=md");
    let resp = reqwest::get(&url)
        .await
        .context("Failed to fetch Google Doc")?;

    if !resp.status().is_success() {
        bail!(
            "Google Doc export failed (HTTP {}). Is the doc link-accessible?",
            resp.status()
        );
    }

    let text = resp.text().await.context("Failed to read response")?;
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_doc_id_parses_edit_urls() {
        let url = "https://docs.google.com/document/d/1a2b3c4d5e6f7g8h9i0j/edit?usp=sharing";
        assert_eq!(extract_doc_id(url).as_deref(), Some("1a2b3c4d5e6f7g8h9i0j"));
    }

    #[test]
    fn extract_doc_id_rejects_non_doc_urls() {
        assert!(extract_doc_id("https://example.com/whatever").is_none());
        assert!(extract_doc_id("https://docs.google.com/document/d/").is_none());
    }
}

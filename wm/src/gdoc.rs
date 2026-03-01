use crate::error::{Result, WmError};

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
        .map_err(|e| WmError::Other(format!("Failed to fetch Google Doc: {e}")))?;

    if !resp.status().is_success() {
        return Err(WmError::Other(format!(
            "Google Doc export failed (HTTP {}). Is the doc link-accessible?",
            resp.status()
        )));
    }

    let text = resp
        .text()
        .await
        .map_err(|e| WmError::Other(format!("Failed to read response: {e}")))?;
    Ok(text)
}

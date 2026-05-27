use std::sync::OnceLock;

/// Glyphs used throughout the UI. Two profiles: emoji (default) and ASCII
/// (for terminals that don't render wide unicode well, or when the user
/// sets `WM_NO_EMOJI`).
pub struct Glyphs {
    pub status_synced: &'static str,
    pub status_local_newer: &'static str,
    pub status_remote_newer: &'static str,
    pub status_conflict: &'static str,
    pub status_not_gisted: &'static str,
    pub dir: &'static str,
    pub file_pane: &'static str,
    pub help: &'static str,
    pub search: &'static str,
    pub warn: &'static str,
    pub info: &'static str,
    pub root: &'static str,
    pub hydrating: &'static str,
    pub welcome: &'static str,
    pub question: &'static str,
    pub git_staged: &'static str,
    pub git_modified: &'static str,
    pub git_untracked: &'static str,
    pub git_clean: &'static str,
}

const EMOJI: Glyphs = Glyphs {
    status_synced: "✅",
    status_local_newer: "⬆️",
    status_remote_newer: "⬇️",
    status_conflict: "❗",
    status_not_gisted: "⚪",
    dir: "📁",
    file_pane: "📄",
    help: "❓",
    search: "🔍",
    warn: "⚠️",
    info: "💬",
    root: "📂",
    hydrating: "🔄",
    welcome: "👋",
    question: "❓",
    git_staged: "✦",
    git_modified: "✱",
    git_untracked: "?",
    git_clean: " ",
};

const ASCII: Glyphs = Glyphs {
    status_synced: "[=]",
    status_local_newer: "[^]",
    status_remote_newer: "[v]",
    status_conflict: "[!]",
    status_not_gisted: "[ ]",
    dir: "[d]",
    file_pane: "[f]",
    help: "[?]",
    search: "[/]",
    warn: "[!]",
    info: "[i]",
    root: "[r]",
    hydrating: "[~]",
    welcome: "[*]",
    question: "[?]",
    git_staged: "+",
    git_modified: "*",
    git_untracked: "?",
    git_clean: " ",
};

static GLYPHS: OnceLock<&'static Glyphs> = OnceLock::new();

/// Get the active glyph set (initialized lazily on first call).
pub fn glyphs() -> &'static Glyphs {
    GLYPHS.get_or_init(|| if use_ascii() { &ASCII } else { &EMOJI })
}

fn use_ascii() -> bool {
    if std::env::var_os("WM_NO_EMOJI").is_some() {
        return true;
    }
    matches!(
        std::env::var("TERM").as_deref(),
        Ok("dumb" | "linux" | "vt100" | "vt220")
    )
}

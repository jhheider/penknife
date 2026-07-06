use std::sync::OnceLock;

/// Glyphs used throughout the UI. Three profiles:
///
/// - slim (default): single-column unicode symbols. Predictable widths, so
///   tree rows and the status bar stay aligned in every terminal/font combo.
/// - emoji: the original wide glyphs, opt-in via `WM_EMOJI=1` for terminals
///   that render them well.
/// - ASCII: pure 7-bit fallback, via `WM_NO_EMOJI` or a dumb `TERM`.
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
    pub welcome: &'static str,
    pub question: &'static str,
    pub git_staged: &'static str,
    pub git_modified: &'static str,
    pub git_untracked: &'static str,
    pub git_clean: &'static str,
}

const SLIM: Glyphs = Glyphs {
    status_synced: "✓",
    status_local_newer: "↑",
    status_remote_newer: "↓",
    status_conflict: "!",
    status_not_gisted: "·",
    dir: "▸",
    file_pane: "≡",
    help: "?",
    search: "/",
    warn: "!",
    info: "i",
    root: "⌂",
    welcome: "»",
    question: "?",
    git_staged: "✦",
    git_modified: "✱",
    git_untracked: "?",
    git_clean: " ",
};

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
    GLYPHS.get_or_init(pick_profile)
}

fn pick_profile() -> &'static Glyphs {
    if use_ascii() {
        &ASCII
    } else if std::env::var_os("WM_EMOJI").is_some() {
        &EMOJI
    } else {
        &SLIM
    }
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

#[cfg(test)]
mod tests {
    use super::SLIM;
    use unicode_width::UnicodeWidthStr;

    /// The whole point of the slim profile is layout stability: every glyph
    /// must occupy exactly one terminal column.
    #[test]
    fn slim_glyphs_are_single_width() {
        for (name, s) in [
            ("status_synced", SLIM.status_synced),
            ("status_local_newer", SLIM.status_local_newer),
            ("status_remote_newer", SLIM.status_remote_newer),
            ("status_conflict", SLIM.status_conflict),
            ("status_not_gisted", SLIM.status_not_gisted),
            ("dir", SLIM.dir),
            ("file_pane", SLIM.file_pane),
            ("help", SLIM.help),
            ("search", SLIM.search),
            ("warn", SLIM.warn),
            ("info", SLIM.info),
            ("root", SLIM.root),
            ("welcome", SLIM.welcome),
            ("question", SLIM.question),
            ("git_staged", SLIM.git_staged),
            ("git_modified", SLIM.git_modified),
            ("git_untracked", SLIM.git_untracked),
            ("git_clean", SLIM.git_clean),
        ] {
            assert_eq!(s.width(), 1, "glyph `{name}` ({s:?}) is not 1 column wide");
        }
    }
}

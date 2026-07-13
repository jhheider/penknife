//! Filesystem helpers that keep the render thread from blocking on cloud
//! storage.
//!
//! macOS File Provider backends (iCloud Drive, Dropbox, OneDrive, ...) leave
//! "online-only" placeholder files on disk: the metadata is local but the
//! bytes live in the cloud. Such a file carries the `SF_DATALESS` flag and has
//! zero data blocks. Calling `read()` on it triggers an on-demand download and
//! *blocks* until the provider materializes it - forever if the provider is
//! offline or unreachable. penknife scans whole synced folders and reads every
//! tracked file to compute sync glyphs, so a single unreachable placeholder
//! used to hang the entire UI before its first frame (unquittable).
//!
//! The rule: never let a passive, bulk read (status glyphs, preview) force a
//! placeholder to materialize. Explicit user actions (push/pull/diff/open in
//! `$EDITOR`) may still trigger a download - that is the user asking for the
//! bytes - but browsing and background refreshes must not.

use std::path::Path;

/// macOS `SF_DATALESS`: "file is a dataless placeholder" (`<sys/stat.h>`). Its
/// bytes are not resident; reading them forces a cloud download.
#[cfg(target_os = "macos")]
const SF_DATALESS: u32 = 0x4000_0000;

/// Pure predicate over a `st_flags` value, split out so it can be unit-tested
/// without a real online-only file (which can't be fabricated portably and is
/// materialized the instant anything reads it).
#[cfg(target_os = "macos")]
fn dataless_flag_set(st_flags: u32) -> bool {
    st_flags & SF_DATALESS != 0
}

/// Is `path` a macOS online-only (dataless) placeholder whose bytes would have
/// to be downloaded on read? `stat()` reports the flag without materializing
/// the file, so this probe is cheap and safe to call while browsing.
///
/// Always `false` off macOS: other platforms' cloud clients (including Linux
/// Dropbox) keep a full local copy, so there is nothing to guard against.
#[cfg(target_os = "macos")]
pub fn is_dataless(path: &Path) -> bool {
    use std::os::macos::fs::MetadataExt;
    // Follow symlinks: a scanned entry may point at the real (dataless) file.
    match std::fs::metadata(path) {
        Ok(meta) => dataless_flag_set(meta.st_flags()),
        // If we can't even stat it, it isn't a readable placeholder; let the
        // normal read path handle (and swallow) the error.
        Err(_) => false,
    }
}

#[cfg(not(target_os = "macos"))]
pub fn is_dataless(_path: &Path) -> bool {
    false
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn dataless_flag_detected() {
        assert!(dataless_flag_set(SF_DATALESS));
        assert!(dataless_flag_set(
            SF_DATALESS | 0x20 /* UF_COMPRESSED */
        ));
    }

    #[test]
    fn ordinary_flags_are_not_dataless() {
        assert!(!dataless_flag_set(0));
        assert!(!dataless_flag_set(0x40 /* UF_TRACKED */));
        assert!(!dataless_flag_set(0x20 /* UF_COMPRESSED */));
    }

    #[test]
    fn regular_file_is_not_dataless() {
        // A file we just wrote is fully resident.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("resident.md");
        std::fs::write(&p, "hello").unwrap();
        assert!(!is_dataless(&p));
    }
}

//! Making sense of pasted text: which links are in it, if any.
//!
//! Reading the clipboard itself is not done here. `iced::clipboard`
//! does it, through the window's own connection to the display server,
//! which is the only reader that works on every desktop oxdm runs on.
//! This module is the pure half: text in, links out.
//!
//! That reader hands over `text/plain` and nothing else. A clipboard
//! actually offers the same content in several formats at once — a
//! browser adds `text/x-moz-url`, a file manager offers only
//! `text/uri-list` — and oxdm used to shell out to `wl-paste` /
//! `xclip` to list them and pick the best. That path is gone: those
//! are separate programs, absent from a default Debian KDE install,
//! and where they were missing the app had no clipboard at all. Plain
//! text everywhere beats a richer read that is sometimes no read.
//!
//! What it costs: files copied in a file manager travel as
//! `text/uri-list` with no plain-text form, so pasting those into oxdm
//! finds nothing. Copying a link — the case this is for — puts the URL
//! in plain text too, from every browser.

/// The single link the Add window starts with, as text.
///
/// A clipboard holding a list is a batch elsewhere; here it is one
/// download, so the field takes the first link on offer. With no link
/// at all, the first non-empty line — a paste that is not a URL is
/// still what the user meant to put there, and the field is theirs to
/// correct.
pub fn first_link_in(text: &str) -> Option<String> {
    if let Some(u) = extract_http_urls(text).into_iter().next() {
        return Some(u.to_string());
    }
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    Some(line.to_owned())
}

/// Find the first http/https URL embedded in `text`. Tolerates leading
/// whitespace, surrounding chatter ("check this: https://..."), and
/// trailing punctuation common in pasted text. Falls back to plain
/// parse so a clean URL with unusual chars still goes through.
fn extract_http_url(text: &str) -> Option<url::Url> {
    let trimmed = text.trim();
    // Whole-string parse only when the string *is* one URL. `Url::parse`
    // strips tabs and newlines per spec, so a two-line list parses as
    // one nonsense URL with the second link glued to the first.
    if !trimmed.contains(char::is_whitespace)
        && let Ok(u) = url::Url::parse(trimmed)
        && matches!(u.scheme(), "http" | "https")
    {
        return Some(u);
    }
    let lower = trimmed.to_ascii_lowercase();
    let start = lower.find("https://").or_else(|| lower.find("http://"))?;
    let tail = &trimmed[start..];
    let end = tail.find(|c: char| c.is_whitespace()).unwrap_or(tail.len());
    let candidate = trim_trailing_punctuation(&tail[..end]);
    let url = url::Url::parse(candidate).ok()?;
    matches!(url.scheme(), "http" | "https").then_some(url)
}

/// Drop the punctuation a URL collects from the sentence around it.
/// A link at the end of a line is followed by the line's full stop,
/// not by a path segment.
fn trim_trailing_punctuation(s: &str) -> &str {
    s.trim_end_matches(|c| {
        matches!(
            c,
            '.' | ',' | ';' | ':' | ')' | ']' | '}' | '"' | '\'' | '>'
        )
    })
}

/// Every http(s) link in `text`, in order, without repeats.
///
/// Scans for the schemes rather than splitting the text into tokens: a
/// pasted list separates its links with whatever it likes — newlines,
/// commas, quotes, `<a href="...">` — and tokenising on whitespace
/// found only the ones that happened to stand alone.
///
/// Lines that are not links are skipped rather than rejected: a pasted
/// page of prose with three URLs in it is three downloads, and a list
/// copied out of a chat window is mostly not links.
pub fn extract_http_urls(text: &str) -> Vec<url::Url> {
    let lower = text.to_ascii_lowercase();
    let mut out: Vec<url::Url> = Vec::new();
    let mut at = 0usize;
    while at < lower.len() {
        let Some(rel) = lower[at..].find("http") else {
            break;
        };
        let start = at + rel;
        // The candidate ends where the next link begins, or at the
        // first character no URL can carry.
        let rest = &text[start..];
        let end = rest
            .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | '|' | '\\'))
            .unwrap_or(rest.len());
        let next = lower[start + 4..]
            .find("http")
            .map(|i| start + 4 + i)
            .unwrap_or(usize::MAX);
        let cut = end.min(next.saturating_sub(start));
        let candidate = trim_trailing_punctuation(&rest[..cut]);
        if let Some(url) = extract_http_url(candidate)
            && !out.iter().any(|u| u == &url)
        {
            out.push(url);
        }
        at = start + 4;
    }
    out
}

/// Links inside a dropped file, if it is a text file that has any.
///
/// Size-capped and read as UTF-8: anything else — an archive, an
/// image, a multi-gigabyte log — is not a list of links, and reading
/// it to find out would be worse than not looking.
pub fn links_in_file(path: &std::path::Path) -> Vec<url::Url> {
    const MAX: u64 = 4 * 1024 * 1024;
    if std::fs::metadata(path).map(|m| m.len()).unwrap_or(u64::MAX) > MAX {
        return Vec::new();
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    extract_http_urls(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A paste is rarely just a link: prose around it, a list with
    /// blank lines, the same link twice from two copies.
    #[test]
    fn every_link_in_the_text_once_in_order() {
        let text = "grab these:\n  https://a.example/one.bin\nnot a link\n\
                    http://b.example/two.zip\nhttps://a.example/one.bin\n";
        let urls = extract_http_urls(text);
        assert_eq!(
            urls.iter().map(|u| u.as_str()).collect::<Vec<_>>(),
            ["https://a.example/one.bin", "http://b.example/two.zip"]
        );
    }

    /// Nothing to add is not an error, and a scheme oxdm cannot fetch
    /// is not a link for this purpose.
    #[test]
    fn text_without_links_yields_none() {
        assert!(extract_http_urls("no links here").is_empty());
        assert!(extract_http_urls("ftp://x.example/f.bin magnet:?xt=z").is_empty());
    }

    /// Two links, however they were separated. A paste does not
    /// promise one per line: comma-separated lists, quoted URLs and
    /// markup all arrive this way, and each of them used to yield a
    /// single link — which then opened the Add dialog for the first
    /// one and lost the rest.
    #[test]
    fn links_are_found_whatever_separates_them() {
        let cases = [
            "https://a.example/1.bin https://b.example/2.bin",
            "https://a.example/1.bin\nhttps://b.example/2.bin",
            "https://a.example/1.bin,https://b.example/2.bin",
            "https://a.example/1.bin; https://b.example/2.bin",
            "<a href=\"https://a.example/1.bin\">one</a><a href=\"https://b.example/2.bin\">two</a>",
            "\"https://a.example/1.bin\" \"https://b.example/2.bin\"",
        ];
        for text in cases {
            let urls = extract_http_urls(text);
            assert_eq!(
                urls.iter().map(|u| u.as_str()).collect::<Vec<_>>(),
                ["https://a.example/1.bin", "https://b.example/2.bin"],
                "{text:?}"
            );
        }
    }

    /// A dropped file is only a link list if it is text with links in
    /// it. Everything else is a file the user wants downloaded, or a
    /// file nobody should be reading to find out.
    #[test]
    fn only_text_files_with_links_are_link_lists() {
        let dir = tempfile::tempdir().unwrap();

        let list = dir.path().join("links.txt");
        std::fs::write(
            &list,
            "https://a.example/one.bin\nhttps://b.example/two.bin\n",
        )
        .unwrap();
        assert_eq!(links_in_file(&list).len(), 2);

        let prose = dir.path().join("notes.txt");
        std::fs::write(&prose, "nothing to download here").unwrap();
        assert!(links_in_file(&prose).is_empty());

        // Not UTF-8: a binary that happens to contain the bytes of a URL
        // is still not a list.
        let binary = dir.path().join("blob.bin");
        std::fs::write(&binary, [0xff, 0xfe, 0x00, 0x01]).unwrap();
        assert!(links_in_file(&binary).is_empty());

        assert!(links_in_file(&dir.path().join("missing.txt")).is_empty());
    }

    /// A clipboard full of links prefills the Add window with one of
    /// them, not with all of them run together. `Url::parse` deletes
    /// newlines rather than rejecting them, so the whole-string parse
    /// used to return `https://a.example/1.binhttps://b.example/2.bin`.
    #[test]
    fn a_list_of_links_reads_as_its_first_link() {
        let text = "https://a.example/1.bin\nhttps://b.example/2.bin\n";
        assert_eq!(
            extract_http_url(text).unwrap().as_str(),
            "https://a.example/1.bin"
        );
        // A lone URL still goes through the whole-string parse, which
        // is what tolerates characters the scanner would cut on.
        assert_eq!(
            extract_http_url("  https://a.example/f.bin  ")
                .unwrap()
                .as_str(),
            "https://a.example/f.bin"
        );
    }

    /// Trailing punctuation belongs to the sentence, not the URL.
    #[test]
    fn a_link_at_the_end_of_a_sentence_keeps_its_path() {
        let urls = extract_http_urls("see https://a.example/f.bin, then stop.");
        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0].as_str(), "https://a.example/f.bin");
    }
}

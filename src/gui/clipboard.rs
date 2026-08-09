//! One-shot clipboard URL detection.

pub fn read_url_from_clipboard() -> Option<url::Url> {
    extract_http_url(&read_text()?)
}

/// Find the first http/https URL embedded in `text`. Tolerates leading
/// whitespace, surrounding chatter ("check this: https://..."), and
/// trailing punctuation common in pasted text. Falls back to plain
/// parse so a clean URL with unusual chars still goes through.
fn extract_http_url(text: &str) -> Option<url::Url> {
    let trimmed = text.trim();
    if let Ok(u) = url::Url::parse(trimmed)
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

/// Every link the clipboard can be read as offering.
///
/// The clipboard is not one string: an owner advertises several
/// targets, and which one answers first decides what the user appears
/// to have copied. `read_text` wants a single URL and takes the first
/// line of the best target — right for a capture, wrong for a paste,
/// where a list of ten links would arrive as one. This reads every
/// textual target in full and keeps whichever yields the most links.
pub fn clipboard_links() -> Vec<url::Url> {
    let mut best: Vec<url::Url> = Vec::new();
    for text in read_texts() {
        let links = extract_http_urls(&text);
        if links.len() > best.len() {
            best = links;
        }
    }
    best
}

/// Every textual payload the clipboard will hand over, in full.
fn read_texts() -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(mut cb) = arboard::Clipboard::new()
        && let Ok(t) = cb.get_text()
    {
        out.push(t);
    }
    #[cfg(target_os = "linux")]
    out.extend(read_linux_texts());
    out
}

pub fn read_text() -> Option<String> {
    if let Ok(mut cb) = arboard::Clipboard::new()
        && let Ok(t) = cb.get_text()
    {
        return Some(t);
    }
    #[cfg(target_os = "linux")]
    {
        if let Some(t) = read_linux_fallback() {
            return Some(t);
        }
    }
    None
}

/// arboard requests a fixed plain-text target. Browsers often only
/// advertise `text/uri-list` / `text/x-moz-url`, so that request
/// fails. X11 / Wayland have no "give me any text" call — the consumer
/// must name a target. So we ask the clipboard owner what it has,
/// then pick the first one whose payload looks textual.
#[cfg(target_os = "linux")]
fn read_linux_fallback() -> Option<String> {
    read_linux_texts()
        .iter()
        .find_map(|payload| first_url_line(payload))
}

/// Every textual target the clipboard owner advertises, in full and in
/// preference order.
///
/// X11 and Wayland have no "give me any text" call — the consumer must
/// name a target — so we ask what is on offer and read each textual one.
/// Whole payloads, not first lines: a `text/uri-list` *is* the list.
#[cfg(target_os = "linux")]
fn read_linux_texts() -> Vec<String> {
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    let Some(targets) = (if wayland {
        run_capture("wl-paste", &["--list-types"])
    } else {
        run_capture("xclip", &["-selection", "clipboard", "-o", "-t", "TARGETS"])
    }) else {
        return Vec::new();
    };

    // Score targets: prefer URL-shaped over generic text so a browser
    // that advertises both gets the canonical URL line, but anything
    // textual still wins over images / files.
    let mut ordered: Vec<(&str, u8)> = targets
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .filter_map(|t| match t {
            "text/uri-list" | "text/x-moz-url" => Some((t, 0)),
            "UTF8_STRING" | "text/plain;charset=utf-8" | "text/plain" => Some((t, 1)),
            "STRING" | "TEXT" => Some((t, 2)),
            _ if t.starts_with("text/") => Some((t, 3)),
            _ => None,
        })
        .collect();
    ordered.sort_by_key(|&(_, p)| p);

    ordered
        .into_iter()
        .filter_map(|(t, _)| {
            if wayland {
                run_capture("wl-paste", &["--no-newline", "-t", t])
            } else {
                run_capture("xclip", &["-selection", "clipboard", "-o", "-t", t])
            }
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn run_capture(prog: &str, args: &[&str]) -> Option<String> {
    use std::process::{Command, Stdio};
    let out = Command::new(prog)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok()
}

/// `text/x-moz-url` is `URL\nTITLE`; `text/uri-list` may include
/// comments prefixed with `#`. Pick the first non-comment, non-empty
/// line.
#[cfg(target_os = "linux")]
fn first_url_line(s: &str) -> Option<String> {
    for line in s.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        return Some(t.to_string());
    }
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
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

    /// Trailing punctuation belongs to the sentence, not the URL.
    #[test]
    fn a_link_at_the_end_of_a_sentence_keeps_its_path() {
        let urls = extract_http_urls("see https://a.example/f.bin, then stop.");
        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0].as_str(), "https://a.example/f.bin");
    }
}

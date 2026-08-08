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
/// Lines that are not links are skipped rather than rejected: a pasted
/// page of prose with three URLs in it is three downloads, and a list
/// copied out of a chat window is mostly not links.
pub fn extract_http_urls(text: &str) -> Vec<url::Url> {
    let mut out: Vec<url::Url> = Vec::new();
    for token in text.split_whitespace() {
        let Some(url) = extract_http_url(trim_trailing_punctuation(token)) else {
            continue;
        };
        if !out.iter().any(|u| u == &url) {
            out.push(url);
        }
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
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    let targets = if wayland {
        run_capture("wl-paste", &["--list-types"])
    } else {
        run_capture("xclip", &["-selection", "clipboard", "-o", "-t", "TARGETS"])
    }?;

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

    for (t, _) in ordered {
        let out = if wayland {
            run_capture("wl-paste", &["--no-newline", "-t", t])
        } else {
            run_capture("xclip", &["-selection", "clipboard", "-o", "-t", t])
        };
        if let Some(o) = out
            && let Some(line) = first_url_line(&o)
        {
            return Some(line);
        }
    }
    None
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

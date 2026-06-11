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
    let mut candidate = &tail[..end];
    while let Some(last) = candidate.chars().last() {
        if matches!(
            last,
            '.' | ',' | ';' | ':' | ')' | ']' | '}' | '"' | '\'' | '>'
        ) {
            candidate = &candidate[..candidate.len() - last.len_utf8()];
        } else {
            break;
        }
    }
    let url = url::Url::parse(candidate).ok()?;
    matches!(url.scheme(), "http" | "https").then_some(url)
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

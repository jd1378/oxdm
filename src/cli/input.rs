//! Turning what was typed into what the daemon takes. Pure, apart from
//! the file reads a caller hands in, so every rule here is testable.

use std::path::{Path, PathBuf};

use base64::Engine;

use super::failure::Failure;
use crate::domain::{
    Algo, AuthAdv, AuthScheme, Checksum, CsSource, CsStatus, SavePathResolver, Settings,
    header_name_eq, upsert_header,
};

/// Reads the text behind `@FILE` or `-i FILE`; `-` is stdin.
pub type ReadSource<'a> = &'a dyn Fn(&str) -> std::io::Result<String>;

/// The URLs in a list file: one per line, blank lines and lines starting
/// with `#` or `//` skipped (the same rules as `odl`).
pub fn url_lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with("//"))
}

/// A link oxdm can download: http or https, nothing else.
pub fn parse_url(s: &str) -> Result<url::Url, Failure> {
    let url = url::Url::parse(s.trim()).map_err(|e| Failure::usage(format!("{s}: {e}")))?;
    match url.scheme() {
        "http" | "https" => Ok(url),
        other => Err(Failure::usage(format!(
            "{s}: only http and https links can be downloaded, not {other}"
        ))),
    }
}

/// Every URL the command names, in order, each once.
pub fn collect_urls(
    args: &[String],
    list: Option<&str>,
    read: ReadSource,
) -> Result<Vec<url::Url>, Failure> {
    let listed = match list {
        Some(path) => read(path).map_err(|e| Failure::usage(format!("{path}: {e}")))?,
        None => String::new(),
    };
    let mut out: Vec<url::Url> = Vec::new();
    for raw in args.iter().map(String::as_str).chain(url_lines(&listed)) {
        let url = parse_url(raw)?;
        if !out.contains(&url) {
            out.push(url);
        }
    }
    Ok(out)
}

/// Request extras from `--header`, split the way the daemon stores
/// them: secrets (cookies, credentials) go to the encrypted columns, the
/// rest are plain headers.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Extras {
    pub headers: indexmap::IndexMap<String, String>,
    pub cookies: Option<String>,
    pub auth: AuthAdv,
}

/// `Name: Value`, or `@FILE` holding one per line (`@-` is stdin, so a
/// secret never has to appear on a command line).
pub fn parse_headers(specs: &[String], read: ReadSource) -> Result<Extras, Failure> {
    let mut out = Extras::default();
    let mut auth_seen = false;
    for spec in specs {
        let lines = match spec.strip_prefix('@') {
            Some(path) => {
                let text = read(path).map_err(|e| Failure::usage(format!("{path}: {e}")))?;
                text.lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty() && !l.starts_with('#'))
                    .map(str::to_owned)
                    .collect()
            }
            None => vec![spec.clone()],
        };
        for line in lines {
            let (name, value) = split_header(&line)?;
            if header_name_eq(name, "Cookie") {
                out.cookies = Some(match out.cookies.take() {
                    Some(prev) => format!("{prev}; {value}"),
                    None => value.to_owned(),
                });
            } else if header_name_eq(name, "Authorization") {
                if auth_seen {
                    return Err(Failure::usage("only one Authorization header can be given"));
                }
                auth_seen = true;
                match parse_authorization(value)? {
                    Some(auth) => out.auth = auth,
                    // A scheme oxdm has no column for travels as it came.
                    None => upsert_header(&mut out.headers, name, value.to_owned()),
                }
            } else {
                upsert_header(&mut out.headers, name, value.to_owned());
            }
        }
    }
    Ok(out)
}

fn split_header(line: &str) -> Result<(&str, &str), Failure> {
    let bad = || Failure::usage(format!("not a header (expected \"Name: Value\"): {line}"));
    let (name, value) = line.split_once(':').ok_or_else(bad)?;
    let name = name.trim();
    let value = value.trim();
    // RFC 9110 token characters; anything else would be refused on the
    // wire, long after the user could fix it.
    let is_token = |c: char| c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c);
    if name.is_empty() || !name.chars().all(is_token) {
        return Err(bad());
    }
    if value.contains(['\r', '\n']) {
        return Err(bad());
    }
    Ok((name, value))
}

/// Basic and Bearer become stored credentials; any other scheme is left
/// to travel as a plain header (`None`).
fn parse_authorization(value: &str) -> Result<Option<AuthAdv>, Failure> {
    let (scheme, rest) = value.split_once(' ').unwrap_or((value, ""));
    let rest = rest.trim();
    if scheme.eq_ignore_ascii_case("bearer") {
        if rest.is_empty() {
            return Err(Failure::usage("Authorization: Bearer needs a token"));
        }
        return Ok(Some(AuthAdv {
            scheme: AuthScheme::Bearer,
            token: rest.to_owned(),
            ..AuthAdv::default()
        }));
    }
    if scheme.eq_ignore_ascii_case("basic") {
        let bad = || Failure::usage("Authorization: Basic needs base64 of \"user:password\"");
        let raw = base64::engine::general_purpose::STANDARD
            .decode(rest)
            .map_err(|_| bad())?;
        let pair = String::from_utf8(raw).map_err(|_| bad())?;
        let (user, password) = pair.split_once(':').ok_or_else(bad)?;
        return Ok(Some(AuthAdv {
            scheme: AuthScheme::Basic,
            username: user.to_owned(),
            password: password.to_owned(),
            ..AuthAdv::default()
        }));
    }
    Ok(None)
}

/// `ALGO:HEX`, e.g. `sha256:9f86…`. Stored as a checksum the user
/// supplied, checked when the download finishes.
pub fn parse_checksum(spec: &str) -> Result<Checksum, Failure> {
    let bad = |why: &str| Failure::usage(format!("--checksum {spec}: {why}"));
    let (algo, hex) = spec
        .split_once(':')
        .ok_or_else(|| bad("expected ALGO:HEX"))?;
    let algo = match algo.trim().to_ascii_lowercase().replace('-', "").as_str() {
        "md5" => Algo::Md5,
        "sha1" => Algo::Sha1,
        "sha256" => Algo::Sha256,
        "sha384" => Algo::Sha384,
        "sha512" => Algo::Sha512,
        _ => return Err(bad("algorithm must be md5, sha1, sha256, sha384 or sha512")),
    };
    let hex = hex.trim().to_ascii_lowercase();
    if hex.len() != algo.hex_len() || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(bad(&format!(
            "a {} digest is {} hex characters",
            algo.label(),
            algo.hex_len()
        )));
    }
    Ok(Checksum {
        algo,
        hash: hex,
        source: CsSource::User,
        status: CsStatus::Unverified,
        expected: None,
    })
}

/// Where a download is saved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dest {
    pub dir: PathBuf,
    /// `None`: the server's name, learned when the download starts.
    pub filename: Option<String>,
    /// The user named the place (`-o`), as opposed to oxdm choosing it
    /// by category.
    pub explicit: bool,
}

/// Read `-o` the way the Add window reads its "Save to" field. With no
/// `-o`, the download saves to `default_dir`.
///
/// `output` must already be absolute: the daemon's working directory is
/// not the caller's.
pub fn destination(
    output: Option<&Path>,
    many: bool,
    default_dir: &Path,
    settings: &Settings,
    is_dir: &dyn Fn(&Path) -> bool,
) -> Result<Dest, Failure> {
    let Some(output) = output else {
        return Ok(Dest {
            dir: default_dir.to_path_buf(),
            filename: None,
            explicit: false,
        });
    };
    // Paths cross to the daemon as JSON strings.
    let text = output
        .to_str()
        .ok_or_else(|| Failure::usage(format!("{}: not valid UTF-8", output.display())))?;
    // Rebuilt from its components: a trailing '/' has said "folder" and
    // has no business in the stored path.
    let tidy = |p: &Path| p.components().collect::<PathBuf>();
    if many {
        return Ok(Dest {
            dir: tidy(output),
            filename: None,
            explicit: true,
        });
    }
    let mut known = settings.known_dirs();
    known.push(default_dir.to_path_buf());
    let d = SavePathResolver {
        fallback_dir: default_dir,
        known_dirs: &known,
        is_dir,
    }
    .resolve(text, None);
    Ok(Dest {
        dir: tidy(&d.dir),
        filename: d.filename,
        explicit: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::failure::Kind;

    fn no_files(_: &str) -> std::io::Result<String> {
        Err(std::io::Error::other("no files in this test"))
    }

    #[test]
    fn a_list_skips_blanks_and_comments_and_repeats() {
        let read = |_: &str| -> std::io::Result<String> {
            Ok("# mirrors\n\nhttps://a.example/x\n// old\n  https://b.example/y  \nhttps://a.example/x\n"
                .to_owned())
        };
        let urls = collect_urls(&["https://c.example/z".into()], Some("list.txt"), &read).unwrap();
        let got: Vec<&str> = urls.iter().map(url::Url::as_str).collect();
        assert_eq!(
            got,
            [
                "https://c.example/z",
                "https://a.example/x",
                "https://b.example/y"
            ]
        );
    }

    #[test]
    fn only_web_links_are_accepted() {
        assert!(parse_url("https://example.com/f").is_ok());
        assert_eq!(
            parse_url("file:///etc/passwd").unwrap_err().kind,
            Kind::Usage
        );
        assert!(parse_url("not a url").is_err());
    }

    /// Credentials go to the encrypted columns; a plain header map is
    /// stored as it is.
    #[test]
    fn secrets_are_split_out_of_the_headers() {
        let specs = [
            "Cookie: a=1".to_owned(),
            "cookie: b=2".to_owned(),
            "Authorization: Bearer tok".to_owned(),
            "X-Api-Version: 3".to_owned(),
        ];
        let x = parse_headers(&specs, &no_files).unwrap();
        assert_eq!(x.cookies.as_deref(), Some("a=1; b=2"));
        assert_eq!(x.auth.scheme, AuthScheme::Bearer);
        assert_eq!(x.auth.token, "tok");
        assert_eq!(x.headers.len(), 1);
        assert_eq!(x.headers["X-Api-Version"], "3");
    }

    #[test]
    fn basic_credentials_are_decoded_into_user_and_password() {
        // base64("user:pa:ss")
        let x =
            parse_headers(&["Authorization: Basic dXNlcjpwYTpzcw==".into()], &no_files).unwrap();
        assert_eq!(x.auth.scheme, AuthScheme::Basic);
        assert_eq!(x.auth.username, "user");
        assert_eq!(x.auth.password, "pa:ss");
        assert!(x.headers.is_empty());
    }

    #[test]
    fn an_unknown_auth_scheme_travels_as_a_header() {
        let x = parse_headers(&["Authorization: Digest abc".into()], &no_files).unwrap();
        assert_eq!(x.auth.scheme, AuthScheme::None);
        assert_eq!(x.headers["Authorization"], "Digest abc");
    }

    #[test]
    fn headers_can_come_from_a_file_so_secrets_stay_off_the_command_line() {
        let read = |p: &str| -> std::io::Result<String> {
            assert_eq!(p, "-");
            Ok("# token\nAuthorization: Bearer from-stdin\n".to_owned())
        };
        let x = parse_headers(&["@-".into()], &read).unwrap();
        assert_eq!(x.auth.token, "from-stdin");
    }

    #[test]
    fn malformed_headers_are_refused() {
        for bad in ["no colon", ": empty name", "Bad Name: x", "X: a\nb"] {
            assert!(
                parse_headers(&[bad.to_owned()], &no_files).is_err(),
                "{bad:?} was accepted"
            );
        }
        let two = [
            "Authorization: Bearer a".into(),
            "Authorization: Bearer b".into(),
        ];
        assert!(parse_headers(&two, &no_files).is_err());
    }

    #[test]
    fn checksums_are_validated_and_lowercased() {
        let c = parse_checksum(&format!("SHA-256:{}", "AB".repeat(32))).unwrap();
        assert_eq!(c.algo, Algo::Sha256);
        assert_eq!(c.hash, "ab".repeat(32));
        assert_eq!(c.source, CsSource::User);
        assert!(parse_checksum("sha256:abc").is_err(), "too short");
        assert!(parse_checksum(&format!("crc32:{}", "a".repeat(8))).is_err());
        assert!(parse_checksum(&format!("md5:{}", "g".repeat(32))).is_err());
    }

    fn settings() -> Settings {
        Settings {
            category_folders: crate::domain::settings::default_category_folders(Path::new("/dl")),
            ..Settings::default()
        }
    }

    #[test]
    fn without_o_the_default_folder_is_used() {
        let s = settings();
        let d = destination(None, false, Path::new("/dl/Agent"), &s, &|_| false).unwrap();
        assert_eq!(d.dir, Path::new("/dl/Agent"));
        assert_eq!(d.filename, None);
        assert!(!d.explicit);
    }

    #[test]
    fn o_names_a_file_or_a_folder() {
        let s = settings();
        let is_dir = |p: &Path| p == Path::new("/home/u/keep");
        let file = destination(
            Some(Path::new("/home/u/keep/x.iso")),
            false,
            Path::new("/dl/Agent"),
            &s,
            &is_dir,
        )
        .unwrap();
        assert_eq!(file.dir, Path::new("/home/u/keep"));
        assert_eq!(file.filename.as_deref(), Some("x.iso"));

        let dir = destination(
            Some(Path::new("/home/u/keep")),
            false,
            Path::new("/dl/Agent"),
            &s,
            &is_dir,
        )
        .unwrap();
        assert_eq!(dir.dir, Path::new("/home/u/keep"));
        assert_eq!(dir.filename, None);

        let fresh = destination(
            Some(Path::new("/home/u/new/")),
            false,
            Path::new("/dl/Agent"),
            &s,
            &is_dir,
        )
        .unwrap();
        // Compared as a path (the separator is the platform's), and as
        // text for the part that matters: nothing trails the name.
        assert_eq!(fresh.dir, Path::new("/home/u/new"));
        assert!(
            !fresh
                .dir
                .to_string_lossy()
                .ends_with(std::path::is_separator),
            "no trailing separator: {}",
            fresh.dir.display()
        );
        assert_eq!(fresh.filename, None);
    }

    /// Several links cannot share one file name.
    #[test]
    fn with_many_links_o_is_always_a_folder() {
        let s = settings();
        let d = destination(
            Some(Path::new("/home/u/x.iso")),
            true,
            Path::new("/dl/Agent"),
            &s,
            &|_| false,
        )
        .unwrap();
        assert_eq!(d.dir, Path::new("/home/u/x.iso"));
        assert_eq!(d.filename, None);
    }
}

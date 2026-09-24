//! Where the command line's words go: stdout for results, stderr for
//! the one error summary, in the format the caller picked.

use std::io::Write;

use clap::ValueEnum;

use super::failure::Failure;
use super::view::Line;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
pub enum Format {
    #[default]
    Text,
    Json,
}

#[derive(Debug, Clone, Copy)]
pub struct Out {
    pub format: Format,
}

impl Out {
    pub fn is_json(self) -> bool {
        self.format == Format::Json
    }

    /// One result: a JSON object on its own line, or its text rendering.
    ///
    /// A closed stdout (a reader that went away, `| head`) is not an
    /// error worth dying over: the download it describes is the
    /// daemon's, and carries on regardless.
    pub fn line(self, line: &Line) {
        let text = match self.format {
            Format::Json => match serde_json::to_string(line) {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!(error = %e, "could not encode output");
                    return;
                }
            },
            Format::Text => line.text(),
        };
        if text.is_empty() {
            return;
        }
        let mut stdout = std::io::stdout().lock();
        let _ = writeln!(stdout, "{text}");
        let _ = stdout.flush();
    }

    /// The single summary of why the command exits non-zero.
    pub fn error(self, f: &Failure) {
        let text = match self.format {
            Format::Json => {
                #[derive(serde::Serialize)]
                struct ErrorLine<'a> {
                    r#type: &'static str,
                    kind: &'static str,
                    message: &'a str,
                    exit_code: u8,
                }
                serde_json::to_string(&ErrorLine {
                    r#type: "error",
                    kind: f.kind.slug(),
                    message: &f.message,
                    exit_code: f.kind.exit_code(),
                })
                .unwrap_or_default()
            }
            Format::Text => format!("oxdm: {}", f.message),
        };
        let _ = writeln!(std::io::stderr().lock(), "{text}");
    }
}

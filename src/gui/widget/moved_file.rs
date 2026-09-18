//! The "this file has moved" notice, shared by every window that
//! offers to open a finished download.
//!
//! The card is message-generic so each window can wrap it in its own
//! modal scaffolding and wire its own dismiss.

use iced::widget::{column, container, row, text};
use iced::{Alignment, Element, Length};

use crate::gui::icons;
use crate::gui::theme::{self, Tokens};
use crate::gui::widget::Btn;

/// A file an Open press asked for and did not find.
///
/// Everything the notice shows is captured at the press, so the card
/// never goes back to disk to re-decide what it is telling the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingFile {
    /// The file's name, as the row shows it.
    pub name: String,
    /// The folder it was last written to. Still openable when it
    /// exists: the file moved, the folder usually did not.
    pub dir: std::path::PathBuf,
    /// Other selected files that are gone too, beyond this one.
    pub others: usize,
}

impl MissingFile {
    pub fn new(file: &std::path::Path, dir: std::path::PathBuf) -> Self {
        Self {
            name: file
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| file.display().to_string()),
            dir,
            others: 0,
        }
    }
}

/// The notice's card: what is gone, where it was, and the one action
/// still worth offering. Callers place it over their own page.
pub fn card<'a, M: Clone + 'a>(
    t: &Tokens,
    missing: &MissingFile,
    on_folder: M,
    on_close: M,
) -> Element<'a, M> {
    let t2 = *t;
    let dir_exists = missing.dir.is_dir();

    let mut card = column![
        row![
            icons::icon("triangle-alert", 20.0, t.status_warning),
            text("This file has moved")
                .font(theme::BODY_BOLD)
                .size(14.0)
                .color(t.fg_1),
        ]
        .spacing(theme::space::S2)
        .align_y(Alignment::Center),
        iced::widget::rich_text::<(), M, _, _>([
            iced::widget::span(missing.name.clone())
                .font(theme::BODY_BOLD)
                .color(t.fg_1),
            iced::widget::span(
                " is not where oxdm saved it. It was moved, renamed or deleted \
                 outside oxdm, so there is nothing at that path to open."
            ),
        ])
        .font(theme::BODY)
        .size(12.0)
        .line_height(1.5)
        .color(t.fg_2),
        container(
            text(missing.dir.display().to_string())
                .font(theme::MONO)
                .size(11.0)
                .color(t.fg_2)
                .wrapping(text::Wrapping::WordOrGlyph),
        )
        .width(Length::Fill)
        .padding(theme::space::S3)
        .style(move |_| container::Style {
            background: Some(t2.bg_sunken.into()),
            border: iced::Border {
                color: t2.border_subtle,
                width: 1.0,
                radius: theme::surface::RADIUS.into(),
            },
            ..Default::default()
        }),
    ]
    .spacing(theme::space::S3);

    if missing.others > 0 {
        card = card.push(
            text(format!(
                "{} other selected {} missing too.",
                missing.others,
                if missing.others == 1 {
                    "file is"
                } else {
                    "files are"
                }
            ))
            .font(theme::BODY)
            .size(11.5)
            .color(t.fg_3),
        );
    }
    if !dir_exists {
        card = card.push(
            text("That folder is gone as well, so there is nowhere to look.")
                .font(theme::BODY)
                .size(11.5)
                .color(t.fg_3),
        );
    }

    card.push(
        row![
            Btn::new(crate::platform::reveal_label())
                .icon("folder")
                .enabled(dir_exists)
                .on_press_maybe(dir_exists.then_some(on_folder))
                .view(t),
            iced::widget::Space::new().width(Length::Fill),
            Btn::new("Close").primary().on_press(on_close).view(t),
        ]
        .spacing(theme::space::S2)
        .align_y(Alignment::Center),
    )
    .into()
}

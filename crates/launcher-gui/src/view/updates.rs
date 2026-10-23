//! Lightforge Updates: the launcher's own release notes.

use crate::*;
use iced::widget::column;

impl Lightforge {
    /// Lightforge's own release notes.
    pub(crate) fn updates_screen(&self) -> Element<'_, Message> {
        let version = env!("CARGO_PKG_VERSION");
        let releases = lightforge_core::releases::all();

        let mut body = column![].spacing(theme::LG);

        if releases.is_empty() {
            // Only reachable in a build whose notes file was emptied; a test
            // in `core::releases` keeps the shipped one honest.
            body = body.push(muted("No release notes in this build.".to_owned(), theme::BODY));
        }

        for release in &releases {
            let heading = match release.version == version {
                true => format!("{} — this version", release.version),
                false => release.version.clone(),
            };
            body = body.push(
                container(
                    column![text(heading).size(theme::TITLE), release_notes(release)]
                        .spacing(theme::MD),
                )
                .padding(theme::LG)
                .width(Length::Fill)
                .style(theme::row),
            );
        }

        // The notes stay readable here whatever the popup has been told, which
        // is what makes "never show this again" a safe thing to tick.
        body = body.extend(
            self.hint(
                "These are the notes the launcher shows after an update. Settings › General \
                 decides whether that popup appears.",
            )
            .map(Element::from),
        );

        // Through `page` like every other page, which is what gives it the
        // same margins and title; built on its own it sat flush against the
        // window's edges.
        self.page(
            "Lightforge Updates",
            Some(format!("You are running {version}.")),
            None,
            body.into(),
        )
    }
}

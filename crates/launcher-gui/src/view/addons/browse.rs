//! Searching the catalogs for addons to install.

use crate::*;
use iced::widget::column;

impl Lightforge {
    pub(crate) fn addons_browse(&self) -> Element<'_, Message> {
        let search = container(
            column![
                row![
                    clickable(
                        "addon search",
                        text_input("Search addons, or paste a project page URL…", &self.search_input)
                            .on_input(Message::SearchChanged)
                            .on_submit(Message::RunSearch)
                            .style(theme::field)
                            .size(theme::BODY)
                            .padding(theme::SM),
                    ),
                    action_button("Search", theme::ACCENT, Message::RunSearch)
                        .enabled(!self.searching && !self.search_input.trim().is_empty()),
                ]
                .spacing(theme::SM)
                .align_y(Alignment::Center),
            ]
            .extend(self.hint(
                "WoWInterface, Tukui and GitHub (type owner/repo). CurseForge needs a \
                 catalog in Settings. A pasted CurseForge, GitHub, WoWInterface or Tukui \
                 page is looked up at that provider — it installs the same way as a \
                 search hit, so a dead link says so rather than failing oddly.",
            ).map(Element::from))
            .spacing(theme::SM),
        )
        .padding(theme::MD)
        .style(theme::card);

        let mut body = column![search].spacing(theme::SM);

        // A provider that could not answer is said out loud: an empty list
        // otherwise reads as "nothing matched".
        for problem in &self.search.problems {
            body = body.push(text(format!("⚠ {problem}")).size(theme::CAPTION).color(theme::UPDATE));
        }
        body = body.extend(self.unavailable_rows(AddonTab::Browse));

        if self.searching {
            body = body.push(muted("searching…".to_owned(), theme::BODY));
        } else if self.search.listings.is_empty() {
            body = body.push(match self.search.query.is_empty() {
                true => empty(
                    "Search for an addon.",
                    "The most-downloaded addons show here once the catalogue has been \
                     fetched. Results come from WoWInterface and Tukui; type owner/repo \
                     for GitHub, or paste an addon's page URL.",
                ),
                false => empty(
                    &format!("Nothing found for “{}”.", self.search.query),
                    "It may be a CurseForge addon, which needs a catalog configured.",
                ),
            });
        }

        // Said once, above the rows: a list nobody asked for has to explain
        // what it is, or it reads as the results of a search that went wrong.
        if self.search.query.is_empty() && !self.search.listings.is_empty() {
            body = body.push(muted("Most downloaded".to_owned(), theme::LABEL));
        }

        let installed_ids: Vec<(&str, &str)> = self
            .addons
            .rows
            .iter()
            .filter_map(|r| r.managed.as_ref())
            .map(|m| (m.provider.as_str(), m.id.as_str()))
            .collect();

        for listing in &self.search.listings {
            let provider = listing.provider.as_str().to_owned();
            // An opted-out project taken from its GitHub instead is installed
            // under that provider, and is just as installed.
            let already = installed_ids.iter().any(|(p, id)| {
                (*p == provider && *id == listing.id)
                    || listing.github.as_deref().is_some_and(|repo| {
                        p.eq_ignore_ascii_case("github") && id.eq_ignore_ascii_case(repo)
                    })
            });

            let mut info = column![
                row![
                    text(listing.name.clone()).size(theme::BODY),
                    pill(provider.clone(), theme::ACCENT),
                ]
                .spacing(theme::SM)
                .align_y(Alignment::Center),
                muted(
                    format!(
                        "{}{}",
                        listing.author.as_deref().unwrap_or("unknown author"),
                        match listing.downloads {
                            0 => String::new(),
                            n => format!(" · {} downloads", downloads_of(n)),
                        }
                    ),
                    theme::CAPTION,
                ),
            ]
            .spacing(theme::XS);
            if listing.opted_out {
                info = info.push(opted_out_note(match listing.github {
                    Some(_) => "the author has disabled third-party downloads, but publishes it on GitHub",
                    None => "the author has disabled third-party downloads",
                }));
            }
            let mut line = row![info].spacing(theme::SM).align_y(Alignment::Center);

            line = line.push(Space::new().width(Length::Fill));
            // "Details" for CurseForge (it has a description), "Page" otherwise.
            match provider.eq_ignore_ascii_case("curse") {
                true => {
                    let open = Message::ShowAddonDetail(
                        provider.clone(),
                        listing.id.clone(),
                        listing.name.clone(),
                    );
                    line = line.push(secondary("Details", open).enabled(true));
                }
                false => {
                    if let Some(url) = &listing.website_url {
                        line = line
                            .push(secondary("Page", Message::OpenUrl(url.clone())).enabled(true));
                    }
                }
            }
            line = line.push(match (already, listing.opted_out, &listing.github) {
                (true, ..) => tag("Installed".to_owned(), theme::OK),
                (false, true, Some(repo)) => action_button(
                    "Get it from GitHub",
                    theme::INSTALL,
                    Message::InstallAddon("github".to_owned(), repo.clone()),
                )
                .enabled(!self.addons_locked()),
                (false, true, None) => tag("Page only".to_owned(), theme::MUTED),
                (false, false, _) => action_button(
                    "Install",
                    theme::INSTALL,
                    Message::InstallAddon(provider.clone(), listing.id.clone()),
                )
                .enabled(!self.addons_locked()),
            });
            body = body.push(container(line).padding(theme::MD).style(theme::row));
        }
        body.into()
    }
}

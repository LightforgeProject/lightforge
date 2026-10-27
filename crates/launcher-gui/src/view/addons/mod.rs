//! The Addons screens: installed, browse, import, export and detail.

use crate::*;
use iced::widget::column;

mod browse;
mod detail;
mod mine;
mod sharing;

impl Lightforge {
    /// Whether any dismissable panel is showing.
    ///
    /// One list, used both to arm the click-anywhere dismissal and to clear
    /// everything in [`Message::Dismiss`]. Adding a panel and forgetting to
    /// name it here is what left the wordmark menu unable to be closed by
    /// clicking the page, so the two now read from the same place.
    /// Whether this provider/id pair is in the ledger for the open game.
    ///
    /// Asked by the detail card twice over, once to pick the opening tab,
    /// once to decide between Install and nothing, so it is one place.
    pub(crate) fn addon_is_installed(&self, provider: &str, id: &str) -> bool {
        self.addons
            .rows
            .iter()
            .filter_map(|r| r.managed.as_ref())
            .any(|m| m.provider == provider && m.id == id)
    }

    /// Clear everything on the addon screen that belongs to one product.
    ///
    /// A search is run against a product's flavor, so Retail's hits are the
    /// wrong answer for Classic and a filter typed over Retail's installed
    /// list hides Classic's. An armed Remove is worse than wrong: it is a
    /// confirmation the user gave about an addon they are no longer looking
    /// at. An in-flight search needs no cancelling, its reply is matched
    /// against the current query and dropped.
    pub(crate) fn reset_addon_screen(&mut self) {
        self.search_input.clear();
        self.search = addonwork::SearchOutcome::default();
        self.addon_filter.clear();
        self.addon_remove_armed = None;
        self.addon_menu_open = None;
        self.addon_auto_note = None;
        // Another product's addons, and their badges with them.
        self.updates.clear();
    }

    /// Addon management: what is installed, and what could be.
    pub(crate) fn addons_screen(&self) -> Element<'_, Message> {
        let tab = |label: &'static str, which: AddonTab| {
            let entry = button(text(label).size(theme::LABEL)).padding(theme::tap([theme::SM, theme::MD]));
            let entry = match self.addon_tab == which {
                true => entry.style(theme::selected),
                false => entry.style(theme::ghost).on_press(Message::AddonTab(which)),
            };
            // The open tab takes the ring too, so the row of tabs can be
            // walked without a gap where the current one is.
            focus::ring(
                focus::Target::Key(format!("addon tab {label}")),
                focus::Press::Send(Message::AddonTab(which)),
                entry,
            )
        };

        let mut body = column![row![
            tab("My Addons", AddonTab::Mine),
            tab("Browse Addons", AddonTab::Browse),
            tab("Import", AddonTab::Import),
            tab("Export", AddonTab::Export),
        ]
        .spacing(theme::XS)]
        .spacing(theme::MD);

        if self.addons_busy {
            let progress = &self.addon_sample;
            body = body.push(
                row![
                    text(progress.line.clone()).size(theme::LABEL),
                    mono(format!("{}/{}", progress.done, progress.total)),
                ]
                .spacing(theme::SM)
                .align_y(Alignment::Center),
            );
        }
        if let Some(e) = &self.error {
            body = body.push(problem(e));
        }

        body = body.push(match self.addon_tab {
            AddonTab::Mine => self.addons_mine(),
            AddonTab::Browse => self.addons_browse(),
            AddonTab::Import => self.addons_import(),
            AddonTab::Export => self.addons_export(),
        });

        // Parent folders only, so the count matches the rows you can see,
        // BigWigs is one addon here and eleven folders on disk.
        let installed = self.addons.rows.iter().filter(|r| r.child_of.is_none()).count();
        let aside = muted(
            match installed {
                1 => "1 addon installed".to_owned(),
                n => format!("{n} addons installed"),
            },
            theme::LABEL,
        );

        self.page("Addons", None, Some(aside.into()), body.into())
    }

    /// What the last update check said about one installed addon, if it is
    /// managed and the check covered it.
    pub(crate) fn update_status(
        &self,
        addon: &addonwork::Row,
    ) -> Option<&lightforge_core::addons::manage::UpdateStatus> {
        let managed = addon.managed.as_ref()?;
        self.updates
            .iter()
            .find(|u| u.id == managed.id && u.provider == managed.provider)
    }

    /// What the last addon run could not install, shown on the tab it was
    /// started from: a refusal from Browse belongs beside the search that
    /// produced it, not on a page of addons it never became.
    pub(crate) fn unavailable_rows(&self, tab: AddonTab) -> Vec<Element<'_, Message>> {
        let Some(report) = self.addon_report.as_ref().filter(|_| self.addon_report_tab == tab) else {
            return Vec::new();
        };
        report
            .unavailable
            .iter()
            .map(|item| {
                let mut line = row![column![
                    text(item.name.clone()).size(theme::BODY),
                    muted(item.reason.clone(), theme::CAPTION),
                ]
                .spacing(theme::XS)]
                .spacing(theme::SM)
                .align_y(Alignment::Center);

                line = line.push(Space::new().width(Length::Fill));
                if let Some(repo) = &item.github {
                    line = line.push(
                        secondary(
                            "Get it from GitHub",
                            Message::InstallAddon("github".to_owned(), repo.clone()),
                        )
                        .enabled(!self.addons_locked()),
                    );
                }
                if let Some(url) = &item.website_url {
                    line = line.push(
                        secondary("Open page", Message::OpenUrl(url.clone())).enabled(true),
                    );
                }
                line = line.push(
                    secondary("Dismiss", Message::DismissUnavailable(item.name.clone())).enabled(true),
                );
                container(line).padding(theme::MD).style(theme::row).into()
            })
            .collect()
    }
}

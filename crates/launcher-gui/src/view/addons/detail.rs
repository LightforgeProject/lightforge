//! One addon's detail card: its description, versions and links.

use crate::*;
use iced::widget::column;

impl Lightforge {
    /// One addon's project page: what it is, and what changed in the build
    /// you would get.
    ///
    /// Tabs: under a long description the changelog would be lost.
    pub(crate) fn detail_card<'a>(&'a self, detail: &'a AddonDetail) -> Element<'a, Message> {
        let title = detail
            .loaded
            .as_ref()
            .map(|d| d.project.name.clone())
            .unwrap_or_else(|| detail.name.clone());
        let mut head = row![text(title).size(theme::TITLE)]
            .spacing(theme::SM)
            .align_y(Alignment::Center);

        if let Some(loaded) = &detail.loaded {
            if loaded.project.download_count > 0 {
                head = head.push(muted(
                    format!("{} downloads", downloads_of(loaded.project.download_count)),
                    theme::CAPTION,
                ));
            }
        }

        let mut body = column![head].spacing(theme::SM).width(Length::Fixed(620.0));

        // The author line sits under the name, not beside it: addons
        // with four maintainers are common and the row would wrap badly.
        if let Some(loaded) = &detail.loaded {
            if !loaded.project.authors.is_empty() {
                body = body.push(muted(
                    format!("by {}", loaded.project.authors.join(", ")),
                    theme::CAPTION,
                ));
            }
            if let Some(summary) = &loaded.project.summary {
                body = body.push(text(summary.clone()).size(theme::BODY));
            }
        }

        match (&detail.loaded, &detail.problem) {
            (None, None) => {
                body = body.push(muted("asking the catalogue…".to_owned(), theme::BODY));
            }
            (_, Some(problem)) => {
                body = body.push(empty(
                    problem.as_str(),
                    "Only CurseForge publishes a description and a changelog.",
                ));
            }
            (Some(loaded), None) => {
                body = body.push(self.detail_tabs(detail, loaded));
                body = body.push(self.detail_body(detail, loaded));
            }
        }

        // The action is here, so nobody has to find the row again.
        let installed = self.addon_is_installed(&detail.provider, &detail.id);

        let mut footer = row![Space::new().width(Length::Fill)].spacing(theme::SM);
        if let Some(url) = detail.loaded.as_ref().and_then(|d| d.project.website_url.clone()) {
            footer = footer.push(secondary("Open the page", Message::OpenUrl(url)).enabled(true));
        }
        // As in the installed row's menu.
        if let Some(url) = detail
            .loaded
            .as_ref()
            .and_then(|d| d.project.source_url.clone().or_else(|| d.project.author_url.clone()))
        {
            footer =
                footer.push(secondary("Support the author", Message::OpenUrl(url)).enabled(true));
        }
        let project = detail.loaded.as_ref().map(|d| &d.project);
        let opted_out = project.is_some_and(|p| p.opted_out);
        let github = project
            .filter(|p| p.opted_out)
            .and_then(|p| p.source_url.as_deref())
            .and_then(lightforge_core::addons::browse::github_repo_of);
        if opted_out {
            body = body.push(
                text(match github {
                    Some(_) => "The author has disabled third-party downloads from CurseForge. \
                                They publish it on GitHub too, which is where Lightforge can get it.",
                    None => "The author has disabled third-party downloads. It can only be \
                             installed from the project page.",
                })
                .size(theme::LABEL)
                .color(theme::UPDATE),
            );
        }
        if !installed {
            let install: Option<Element<'static, Message>> = match (opted_out, github) {
                (false, _) => Some(
                    action_button(
                        "Install",
                        theme::INSTALL,
                        Message::InstallAddon(detail.provider.clone(), detail.id.clone()),
                    )
                    .enabled(!self.addons_locked()),
                ),
                (true, Some(repo)) => Some(
                    action_button(
                        "Get it from GitHub",
                        theme::INSTALL,
                        Message::InstallAddon("github".to_owned(), repo),
                    )
                    .enabled(!self.addons_locked()),
                ),
                (true, None) => None,
            };
            if let Some(install) = install {
                footer = footer.push(install);
            }
        }
        footer = footer.push(secondary("Close", Message::CloseAddonDetail).enabled(true));
        body = body.push(footer);

        container(container(body).padding(theme::LG).style(theme::floating))
            .center(Length::Fill)
            .padding(theme::XL)
            .into()
    }

    /// The two tab buttons, each naming what it holds.
    ///
    /// An empty tab is disabled, not hidden: that is an answer too.
    pub(crate) fn detail_tabs<'a>(
        &'a self,
        detail: &'a AddonDetail,
        loaded: &'a addonwork::Detail,
    ) -> Element<'a, Message> {
        let tab = |label: String, which: DetailTab, available: bool| {
            let selected = detail.tab == which;
            let key = format!("detail tab {label}");
            let tab = button(text(label).size(theme::BODY))
                .padding([theme::XS, theme::MD])
                .style(match selected {
                    true => theme::selected,
                    false => theme::ghost,
                });
            tappable(key, (available && !selected).then_some(Message::AddonDetailTab(which)), tab)
        };

        let changelog_label = match &loaded.changelog_build {
            Some(build) => format!("What changed in {build}"),
            None => "What changed".to_owned(),
        };
        row![
            tab(
                changelog_label,
                DetailTab::Changelog,
                loaded.changelog.as_ref().is_some_and(|d| !d.is_empty()),
            ),
            tab(
                "About".to_owned(),
                DetailTab::About,
                loaded.description.as_ref().is_some_and(|d| !d.is_empty()),
            ),
        ]
        .spacing(theme::XS)
        .into()
    }

    /// Whichever document the open tab names, scrolled.
    pub(crate) fn detail_body<'a>(
        &'a self,
        detail: &'a AddonDetail,
        loaded: &'a addonwork::Detail,
    ) -> Element<'a, Message> {
        let (document, missing) = match detail.tab {
            DetailTab::Changelog => (
                loaded.changelog.as_ref(),
                "This build ships no changelog — plenty of authors never write one.",
            ),
            DetailTab::About => (
                loaded.description.as_ref(),
                "This addon has no description beyond its summary.",
            ),
        };

        let Some(document) = document.filter(|d| !d.is_empty()) else {
            return empty(missing, "The CurseForge page has whatever else there is.");
        };

        let mut content = column![].spacing(theme::SM);
        for block in &document.blocks {
            content = content.push(rich_block(block));
        }
        if document.truncated {
            content = content.push(muted(
                "This is longer than Lightforge shows — the rest is on the page.".to_owned(),
                theme::CAPTION,
            ));
        }

        focus::region(
            "addon detail",
            scrollable(container(content).padding(theme::MD))
                .height(Length::Fixed(320.0))
                .style(theme::scrollbar),
        )
    }
}

//! The Settings page, one section at a time.

use crate::*;
use iced::widget::column;

mod builds;
mod drives;
mod general;
mod setup;

impl Lightforge {
    /// Settings: every section on one page, with an index down the left
    /// that jumps to each and marks the one being read.
    ///
    /// Rows share one width.
    pub(crate) fn settings(&self) -> Element<'_, Message> {
        let mut nav = column![].spacing(theme::XS).width(Length::Fixed(SETTINGS_NAV_WIDTH));
        for tab in SettingsTab::ALL {
            // Pressable while marked too: the mark follows the scrolling, so
            // the marked entry is not necessarily where its section starts.
            let entry = button(text(tab.label()).size(theme::tap_text(theme::BODY)))
                .width(Length::Fill)
                .padding(theme::tap([theme::SM, theme::MD]))
                .on_press(Message::SettingsTab(tab))
                .style(match self.settings_tab == tab {
                    true => theme::selected,
                    false => theme::ghost,
                });
            nav = nav.push(self.ring(focus::Target::SettingsTab(tab), Some(Message::SettingsTab(tab)), entry));
        }

        // The error leads, where it is seen. Sections take the full width;
        // rows right-align their controls.
        let mut content = column![].spacing(theme::XL).width(Length::Fill);
        if let Some(e) = &self.error {
            content = content.push(problem(e));
        }
        for tab in SettingsTab::ALL {
            let body = match tab {
                SettingsTab::Setup => self.setup_section(),
                SettingsTab::General => self.general_section(),
                SettingsTab::Proton => self.proton_section(),
                SettingsTab::BattleNet => self.battle_net_section(),
                SettingsTab::Downloads => self.downloads_section(),
                SettingsTab::Addons => self.addons_section(),
                SettingsTab::Storage => self.storage_section(),
                SettingsTab::About => self.about_section(),
            };
            let body = section(tab.label(), body);
            content = content.push(container(body).id(tab.anchor()).width(Length::Fill));
        }
        // Room to scroll the last section's heading up to where the others
        // land, most of the time; [`SettingsLayout::reading`] covers the rest.
        content = content.push(Space::new().height(Length::Fixed(theme::XL * 4.0)));

        column![
            self.header(),
            row![
                focus::group("settings nav", nav),
                container(Space::new().width(Length::Fixed(1.0)))
                    .height(Length::Fill)
                    .style(theme::divider),
                focus::pane(
                    SETTINGS_PAGE,
                    scrollable(container(content).padding([0.0, theme::LG]))
                        .id(SETTINGS_PAGE)
                        .on_scroll(Message::SettingsScrolled)
                        .width(Length::Fill)
                        .height(Length::Fill),
                ),
            ]
            .spacing(theme::LG)
            .padding(theme::LG),
        ]
        .into()
    }

    /// Where one product stands on the build the in-game shop needs.
    pub(crate) fn shop_build_state(&self, code: &ProductCode) -> ShopBuild {
        if Kind::of(code) != Kind::Managed {
            return ShopBuild::NotWow;
        }
        match self.config.effective_proton(code) == Some(proton::SHOP_BUILD) {
            true => ShopBuild::Running,
            false => ShopBuild::Elsewhere,
        }
    }

    /// The build a product's row resolves to, pin or no pin.
    ///
    /// A pin wins; the fallback is the app default, or Battle.net's for a hand-off.
    pub(crate) fn row_build(&self, code: &ProductCode) -> Option<&proton::ProtonRelease> {
        let pin = match Kind::of(code) {
            Kind::Managed => self.config.effective_proton(code),
            Kind::HandOff => self.config.handoff_proton(code),
        };
        let installed = self.setup.as_ref().map(|s| s.installed.as_slice())?;
        proton::select(installed, pin)
    }

    /// How a product's build sits against the prefix every product shares.
    pub(crate) fn row_fit(&self, code: &ProductCode) -> proton::PrefixFit {
        let Some(build) = self.row_build(code) else {
            return proton::PrefixFit::Unknown;
        };
        proton::prefix_fit(
            &build.version,
            self.setup.as_ref().and_then(|s| s.prefix.built_by.as_deref()),
        )
    }

    /// The games this page has anything to say about: every configured
    /// product that is actually installed.
    ///
    /// Installed, by Battle.net's `product.db`, not merely configured.
    pub(crate) fn installed_products(&self) -> Vec<(ProductCode, String)> {
        self.config
            .products
            .iter()
            .filter(|p| self.installed_versions.contains_key(p.code.as_str()))
            .map(|p| (p.code.clone(), p.display_name.clone()))
            .collect()
    }
}

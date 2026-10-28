//! The Settings page: its tabs, where each section sits, and the jump to one.

use crate::*;

/// A tab of the "Install Proton Version" window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ProtonCategory {
    Steam,
    GloriousEggroll,
    CachyOs,
}

impl ProtonCategory {
    pub(crate) const ALL: [Self; 3] = [Self::Steam, Self::GloriousEggroll, Self::CachyOs];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Steam => "Steam Proton",
            Self::GloriousEggroll => "Proton-GE",
            Self::CachyOs => "Proton-CachyOS",
        }
    }

    /// The downloadable family behind the tab, if there is one.
    pub(crate) fn family(self) -> Option<proton::ProtonFamily> {
        match self {
            Self::GloriousEggroll => Some(proton::ProtonFamily::GloriousEggroll),
            Self::CachyOs => Some(proton::ProtonFamily::CachyOs),
            Self::Steam => None,
        }
    }
}

/// A section of the Settings page, and its entry in the index beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum SettingsTab {
    General,
    Setup,
    Proton,
    BattleNet,
    Downloads,
    Addons,
    Storage,
    About,
}

impl SettingsTab {
    /// In page order: the switches people change most first, then the
    /// longer cards, then the ones visited once.
    pub(crate) const ALL: [Self; 8] = [
        Self::General,
        Self::Downloads,
        Self::Addons,
        Self::BattleNet,
        Self::Proton,
        Self::Storage,
        Self::Setup,
        Self::About,
    ];

    /// The id of the section's container on the page, which is how it is
    /// found to be jumped to.
    pub(crate) fn anchor(self) -> &'static str {
        match self {
            Self::General => "settings General",
            Self::Setup => "settings Setup",
            Self::Proton => "settings Proton",
            Self::BattleNet => "settings Battle.net",
            Self::Downloads => "settings Downloads",
            Self::Addons => "settings Addons",
            Self::Storage => "settings Storage",
            Self::About => "settings About",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Setup => "Setup",
            Self::Proton => "Proton",
            Self::BattleNet => "Battle.net",
            Self::Downloads => "Downloads",
            Self::Addons => "Addons",
            Self::Storage => "Storage",
            Self::About => "About",
        }
    }
}

/// Where the Settings sections sat when the page was last laid out.
///
/// In layout coordinates, the page's own, as [`focus`] records controls.
#[derive(Debug, Clone, Default)]
pub(crate) struct SettingsLayout {
    /// Where the page's content starts.
    pub(crate) top: f32,
    /// In page order, which is the order the measuring found them in.
    pub(crate) sections: Vec<(SettingsTab, iced::Rectangle)>,
}

impl SettingsLayout {
    pub(crate) fn bounds(&self, tab: SettingsTab) -> Option<iced::Rectangle> {
        self.sections.iter().find(|(t, _)| *t == tab).map(|(_, bounds)| *bounds)
    }

    /// How far down the page `tab` starts.
    pub(crate) fn offset(&self, tab: SettingsTab) -> Option<f32> {
        self.bounds(tab).map(|bounds| bounds.y - self.top)
    }

    /// The section someone looking at `viewport` is reading, given that the
    /// index marks `marked`.
    pub(crate) fn reading(&self, viewport: &scrollable::Viewport, marked: SettingsTab) -> Option<SettingsTab> {
        self.reading_at(
            viewport.absolute_offset().y,
            viewport.bounds().height,
            viewport.content_bounds().height,
            marked,
        )
    }

    /// [`SettingsLayout::reading`] for a page scrolled to `y`, showing
    /// `shown` of `height`.
    ///
    /// The section under a reading line near the top, which slides down over
    /// the last screenful, where short sections never reach the top. At the
    /// bottom, a marked section keeps its mark while its heading shows.
    pub(crate) fn reading_at(&self, y: f32, shown: f32, height: f32, marked: SettingsTab) -> Option<SettingsTab> {
        let max = (height - shown).max(0.0);
        if y >= max - 1.0 && self.offset(marked).is_some_and(|top| top >= y && top < y + shown) {
            return Some(marked);
        }
        let near_end = match shown > 0.0 {
            true => (1.0 - (max - y) / shown).clamp(0.0, 1.0),
            false => 0.0,
        };
        let line = y + theme::LG + near_end * (shown - 2.0 * theme::LG).max(0.0);
        self.sections
            .iter()
            .rev()
            .find(|(_, bounds)| bounds.y - self.top <= line)
            .or(self.sections.first())
            .map(|(tab, _)| *tab)
    }
}

/// A section a jump scrolled to, kept at the top while the page settles.
pub(crate) struct SettingsAnchor {
    pub(crate) tab: SettingsTab,
    /// The offset it was last scrolled to.
    pub(crate) y: f32,
    /// The one before, when the page grew and it had to move again.
    pub(crate) prior: Option<f32>,
    pub(crate) since: Instant,
}

/// Find where every Settings section sits on the page.
pub(crate) fn measure_settings() -> Task<Message> {
    use iced::advanced::widget::operation::{Outcome, Scrollable};
    use iced::advanced::widget::{Id, Operation};

    struct Measure(SettingsLayout);

    impl Operation<SettingsLayout> for Measure {
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<SettingsLayout>)) {
            operate(self);
        }

        fn container(&mut self, id: Option<&Id>, bounds: iced::Rectangle) {
            let Some(id) = id else { return };
            if let Some(tab) = SettingsTab::ALL.into_iter().find(|tab| *id == Id::new(tab.anchor())) {
                self.0.sections.push((tab, bounds));
            }
        }

        fn scrollable(
            &mut self,
            id: Option<&Id>,
            _bounds: iced::Rectangle,
            content_bounds: iced::Rectangle,
            _translation: iced::Vector,
            _state: &mut dyn Scrollable,
        ) {
            if id == Some(&Id::new(SETTINGS_PAGE)) {
                self.0.top = content_bounds.y;
            }
        }

        fn finish(&self) -> Outcome<SettingsLayout> {
            Outcome::Some(self.0.clone())
        }
    }

    iced::advanced::widget::operate(Measure(SettingsLayout::default())).map(Message::SettingsMeasured)
}

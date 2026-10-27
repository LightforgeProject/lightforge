//! Proton builds: what is installed, what each game runs on, installing more.

use crate::*;
use iced::widget::column;

impl Lightforge {
    /// Every Proton build on the machine, and how to add or remove one.
    ///
    /// Installing lives in this card's own header rather than behind the
    /// update check in the card above, which is where it used to be: the
    /// button that revealed the picker and the picker it revealed were in
    /// two different sections, so the page told you to go and press
    /// something somewhere else.
    pub(crate) fn proton_builds(&self) -> Element<'_, Message> {
        let mut rows = column![].spacing(theme::SM);

        let installed = self.setup.as_ref().map(|s| s.installed.as_slice()).unwrap_or_default();
        if installed.is_empty() {
            rows = rows.push(muted("none found yet".to_owned(), theme::LABEL));
        }

        for build in installed {
            let pinned_by = self.config.pinned_to(&build.version);
            let managed = build.source == lightforge_core::proton::ProtonSource::Managed;

            let mut facts = row![
                mono(build.version.clone()),
                dot(),
                muted(lightforge_core::setup::describe_source(build).to_owned(), theme::CAPTION),
            ]
            .spacing(theme::SM)
            .align_y(Alignment::Center);

            // Absent until the measuring task answers, rather than a zero
            // that would read as an empty directory.
            if let Some(size) = self.proton_sizes.get(&build.path) {
                facts = facts.push(dot());
                facts = facts.push(muted(bytes_of(*size, 0), theme::CAPTION));
            }

            // Only managed builds are ours to delete, and only when nothing
            // points at them, the reason is shown instead of a dead button,
            // since "why can't I remove this" is the whole question here.
            //
            // The count, not the list. Spelled out it ran to two lines of
            // full product names, "pinned by the app default, Battle.net,
            // World of Warcraft, World of Warcraft Classic, World of
            // Warcraft: Forever, Beta", which is a paragraph where a row
            // wants a word. The names are still there on hover, because the
            // count alone does not tell you what to go and unpin.
            // A button in every case, pressable or not, so the column on the
            // right reads as one: what cannot be done is a greyed button that
            // says why, not a loose line of text.
            let idle = |label: String| {
                secondary(&label, Message::RemoveProton(build.version.clone()))
                    .keyed(format!("proton idle {}", build.version), false)
            };
            let action: Element<Message> = match (managed, pinned_by.is_empty()) {
                (false, _) => idle("Not managed here".to_owned()),
                (true, false) => tooltip(
                    idle(match pinned_by.len() {
                        1 => "In use by 1".to_owned(),
                        n => format!("In use by {n}"),
                    }),
                    container(text(pinned_by.join(", ")).size(theme::LABEL))
                        .padding([theme::XS, theme::SM])
                        .style(theme::floating),
                    tooltip::Position::Left,
                )
                .into(),
                (true, true) => secondary("Remove", Message::RemoveProton(build.version.clone()))
                    .enabled(!self.setup_running),
            };

            rows = rows.push(
                container(
                    row![facts, Space::new().width(Length::Fill), action]
                        .spacing(theme::MD)
                        .align_y(Alignment::Center),
                )
                .padding([theme::SM, theme::MD])
                .style(theme::row),
            );
        }

        // While a build is coming down, in this card. It used to throw the
        // user to Settings > Setup, because that was the only place a
        // progress bar existed, pressing Install and landing on a different
        // page reads as a misclick.
        if self.setup_running {
            rows = rows.push(self.install_progress());
        }

        rows.into()
    }

    /// The build being installed: what setup has said so far and a bar.
    pub(crate) fn install_progress(&self) -> Element<'_, Message> {
        let mut working = column![].spacing(theme::XS);
        for line in &self.setup_sample.log {
            working = working.push(muted(line.clone(), theme::CAPTION));
        }
        working = working.push(match self.setup_sample.fraction() {
            Some(fraction) => Element::from(progress_bar(0.0..=1.0, fraction).girth(6)),
            // No content-length: the bytes are the only honest figure.
            None => mono(bytes_of(self.setup_sample.done, 0)).into(),
        });
        container(working).padding(theme::MD).style(theme::card).into()
    }

    /// The control that adds a build, for the Builds card's header: one
    /// button, the window behind it has the lists.
    pub(crate) fn proton_install(&self) -> Element<'_, Message> {
        secondary("Install Proton Version", Message::OpenProtonInstaller)
            .enabled(!self.setup_running)
    }

    /// Which Proton each game runs under, and the update check.
    ///
    /// Every installed game, not the one the Play page is showing: Settings is
    /// about the install as a whole, and a row that renamed itself as you
    /// switched tiles read as a bug, correctly, since nothing on the page
    /// said which game it meant.
    pub(crate) fn proton_section(&self) -> Element<'_, Message> {
        let games = self.installed_products();
        // Two questions: which Proton each game runs, and which Protons are
        // on this machine.
        column![
            group_label("Games", None),
            setting_group(self.game_pins(&games)),
            group_label("When a game has no pin", None),
            setting_group(self.fallback_pins()),
            self.pin_notes(&games),
            group_label("Installed builds", Some(self.proton_install())),
            setting_card(self.proton_builds()),
        ]
        .spacing(theme::SM)
        .into()
    }

    /// A build picker for one row.
    fn proton_picker(
        &self,
        key: String,
        unpinned: Unpinned,
        current: Option<String>,
        on_pick: impl Fn(ProtonChoice) -> Message,
    ) -> Element<'_, Message> {
        // Built per row rather than once: the first entry is "no pin of my
        // own", and what that falls through to is different on a title
        // Battle.net opens than on a game Lightforge launches itself.
        let builds = std::iter::once(ProtonChoice(None, unpinned))
            .chain(
                self.setup
                    .iter()
                    .flat_map(|s| s.installed.iter())
                    .map(|build| ProtonChoice(Some(build.version.clone()), unpinned)),
            )
            .collect();
        self.picker(
            key,
            builds,
            Some(ProtonChoice(current, unpinned)),
            "",
            on_pick,
            Length::Fixed(260.0),
            theme::BODY,
        )
    }

    /// One row per installed game, in the order the config lists them, the
    /// same order as the Play page's tiles, so the two read alike.
    fn game_pins(&self, games: &[(ProductCode, String)]) -> Vec<Element<'_, Message>> {
        let mut pins: Vec<Element<'_, Message>> = Vec::new();
        for (code, label) in games {
            // Every game, a title Battle.net launches included. Its pin is
            // honoured on the hand-off path too, so the control is not a lie,
            // the client is told to start *that* game, and that game is
            // what the pin is about. What it cannot do is give the title a
            // prefix of its own: there is one, shared, which is what the
            // downgrade sign is for.
            let pin = self
                .config
                .product(code)
                .and_then(|p| p.profile.proton_ge_version.clone());
            let for_code = code.clone();

            // Both signs now, where the old row had space for one and the
            // shop always won. They say different things, one names a build
            // to move to, the other warns about the prefix this one shares,
            // and a game can be in both states at once.
            let mut tags: Vec<Element<'_, Message>> = Vec::new();
            match self.shop_build_state(code) {
                ShopBuild::Running => tags.push(tag("In-game shop working".to_owned(), theme::OK)),
                ShopBuild::Elsewhere => {
                    tags.push(tag(format!("{} recommended", proton::SHOP_BUILD), theme::UPDATE))
                }
                ShopBuild::NotWow => {}
            }
            if self.row_fit(code).is_downgrade() {
                tags.push(tag("Older than this prefix".to_owned(), theme::UPDATE));
            }

            pins.push(pin_line(
                label.clone(),
                match Kind::of(code) {
                    Kind::Managed => "Started by Lightforge",
                    Kind::HandOff => "Started by Battle.net",
                },
                tags,
                self.proton_picker(
                    format!("proton {}", code.as_str()),
                    match Kind::of(code) {
                        Kind::Managed => Unpinned::Default,
                        Kind::HandOff => Unpinned::BattleNet,
                    },
                    pin,
                    move |choice| Message::PinProductProton(for_code.clone(), choice),
                ),
            ));
        }

        // Nothing installed is worth saying: an empty card looks like a page
        // that failed to load.
        if games.is_empty() {
            pins.push(setting("No games yet", "Games appear here once they are installed.", None, Space::new()));
        }
        pins
    }

    /// The two rows the games fall back to: Lightforge's default and Battle.net's.
    fn fallback_pins(&self) -> Vec<Element<'_, Message>> {
        // A group of their own, because they are not games. The rows above
        // are things you launch; these two are what they fall back to.
        vec![
            pin_line(
                "Default".to_owned(),
                "Every game Lightforge starts without a pin of its own.",
                Vec::new(),
                self.proton_picker(
                    "proton default".to_owned(),
                    Unpinned::Newest,
                    self.config.default_proton.version().map(str::to_owned),
                    Message::PinDefaultProton,
                ),
            ),
            pin_line(
                "Battle.net".to_owned(),
                "Battle.net, Agent, and the games it starts without a pin.",
                // The prefix's own build, beside the pin that will next open it.
                // The one fact that makes the signs above legible.
                vec![match self.setup.as_ref().and_then(|s| s.prefix.built_by.as_deref()) {
                    Some(built_by) => muted(format!("prefix built by {built_by}"), theme::CAPTION),
                    None => muted("no prefix yet".to_owned(), theme::CAPTION),
                }
                .into()],
                self.proton_picker(
                    "proton battle.net".to_owned(),
                    Unpinned::Default,
                    self.config.battle_net_proton.version().map(str::to_owned),
                    Message::PinBattleNetProton,
                ),
            ),
        ]
    }

    /// What the pins mean, a downgrade they cause, and the shop's build.
    fn pin_notes(&self, games: &[(ProductCode, String)]) -> Element<'_, Message> {
        // One line for the whole list rather than one under each game: the
        // reason is the same every time it applies.
        let shop = games
            .iter()
            .any(|(code, _)| matches!(self.shop_build_state(code), ShopBuild::Elsewhere));

        // Said once, where the pin that causes it is set, and not behind the
        // hints toggle: this one has a cost to games the user is not looking at.
        let downgrade: Vec<String> = games
            .iter()
            .filter(|(code, _)| self.row_fit(code).is_downgrade())
            .map(|(_, label)| label.clone())
            .collect();
        column![]
            .spacing(theme::SM)
            .extend(self.hint(
                "A game with no pin of its own follows the default. Battle.net and Agent \
                 are a different workload from the game, so they pin separately — a game \
                 Battle.net starts for you follows the game's own pin where it has one, \
                 and Battle.net's where it does not.",
            ).map(Element::from))
            .extend((!downgrade.is_empty()).then(|| {
                text(format!(
                    "⚠ {} {} pinned to a build older than the {} that last upgraded the \
                     prefix. There is one prefix for everything here, and Proton only \
                     migrates it forward, so an older build may leave it in a state the \
                     newer one did not expect. You will be asked again at launch.",
                    downgrade.join(", "),
                    match downgrade.len() {
                        1 => "is",
                        _ => "are",
                    },
                    self.setup
                        .as_ref()
                        .and_then(|s| s.prefix.built_by.clone())
                        .unwrap_or_default(),
                ))
                .size(theme::LABEL)
                .color(theme::UPDATE)
            }).map(Element::from))
            .extend(
                shop.then(|| {
                    self.hint(format!(
                        "World of Warcraft's in-game shop and support pages only render \
                         under {}. Newer builds start the game's embedded browser and lose \
                         its GPU process, which is the grey box instead of a page. \
                         Everything else about the game is unaffected — install it under \
                         Builds below, then pin it here.",
                        proton::SHOP_BUILD,
                    ))
                })
                .flatten()
                .map(Element::from),
            )
            .into()
    }
}

/// A row: the name and what starts it take the slack, then any signs,
/// then the picker, a fixed width so every row's control lines up.
fn pin_line<'a>(
    label: String,
    about: &'static str,
    tags: Vec<Element<'a, Message>>,
    control: Element<'a, Message>,
) -> Element<'a, Message> {
    setting(
        label,
        about,
        None,
        row![].extend(tags).push(control).spacing(theme::SM).align_y(Alignment::Center),
    )
}

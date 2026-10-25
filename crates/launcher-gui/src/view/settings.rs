//! The Settings page, one section at a time.

use crate::*;
use iced::widget::column;

impl Lightforge {
    /// The 32-bit runtime, when it is missing.
    ///
    /// Battle.net's installer is the first 32-bit binary in setup, so without
    /// a 32-bit userspace everything looks fine until the last step dies.
    /// Said here, before anything downloads. The command is offered to copy,
    /// not run: a launcher running a package manager as root on a guess is a
    /// trust escalation, and the systems that most need help (SteamOS,
    /// rpm-ostree, NixOS) are the ones such a button could not serve.
    pub(crate) fn runtime_row() -> Element<'static, Message> {
        let remedy = lightforge_core::runtime::remedy();

        let mut lines = column![
            text("32-bit runtime").size(theme::BODY),
            mono("missing — Battle.net's installer cannot run without it".to_owned()),
        ]
        .spacing(theme::XS);

        if let Some(command) = &remedy.command {
            lines = lines.push(muted(
                format!("on {}, run:", remedy.distro),
                theme::CAPTION,
            ));
            lines = lines.push(mono(command.clone()));
        }
        if let Some(note) = &remedy.note {
            lines = lines.push(muted(note.clone(), theme::CAPTION));
        }
        if remedy.command.is_none() && remedy.note.is_none() {
            lines = lines.push(muted(
                lightforge_core::runtime::ADVICE.to_owned(),
                theme::CAPTION,
            ));
        }

        // Only offer a copy when there is something to copy. A dead button
        // beside an immutable distro's instructions would be worse than none.
        let mut actions = row![].spacing(theme::SM).align_y(Alignment::Center);
        if let Some(command) = remedy.command.clone() {
            actions = actions.push(
                action_button(
                    "Copy command",
                    theme::ACCENT,
                    Message::CopyRuntimeCommand(command),
                )
                .enabled(true),
            );
        }
        actions = actions.push(secondary("Re-check", Message::RecheckRuntime).enabled(true));

        container(
            row![
                container(
                    container(Space::new().width(Length::Fixed(8.0)).height(Length::Fixed(8.0)))
                        .style(theme::dot(theme::ERROR)),
                )
                .padding(theme::SM)
                .style(theme::badge(theme::ERROR)),
                lines.width(Length::Fill),
                actions,
            ]
            .spacing(theme::MD)
            .align_y(Alignment::Center),
        )
        .width(Length::Fill)
        .padding(theme::MD)
        .style(theme::row)
        .into()
    }

    /// First-run provisioning: what is missing, and one button to fix it.
    /// Proton, the prefix and Battle.net: what is there, and the button that
    /// provisions whatever isn't.
    pub(crate) fn setup_section(&self) -> Element<'_, Message> {
        self.setup_steps(false)
    }

    /// The same list inside the first-run wizard, which owns two of the things
    /// this draws for itself: where to install, and whether Battle.net is
    /// wanted. Drawing them twice on one slide is how the wizard ended up with
    /// two "Install to" fields and two ways to say no to Battle.net.
    pub(crate) fn setup_section_in_wizard(&self) -> Element<'_, Message> {
        self.setup_steps(true)
    }

    pub(crate) fn setup_steps(&self, in_wizard: bool) -> Element<'_, Message> {
        let mut body = column![]
            .extend(self.hint(
                "Proton, the Wine prefix and Battle.net. Each step is skipped if it is \
                 already done.",
            ).map(Element::from))
            .spacing(theme::SM);

        // Where all three steps below install to. Only offered while nothing
        // is installed: after that, moving it is a data migration rather than
        // a setting; see `relocate`. Once the prefix is built, the Wine
        // prefix step below says where it is, so nothing is lost by hiding it.
        if self.library_empty && !in_wizard {
            body = body.push(setting_group([
                setting(
                    "Install location",
                    "Battle.net and the Wine prefix go here. Games can go on any drive.",
                    None,
                    row![
                        clickable(
                            "install location",
                            text_input("~/Games/Blizzard", &self.location_input)
                                .on_input(Message::LocationChanged)
                                .on_submit(Message::UseLocation)
                                .style(theme::field)
                                .size(theme::BODY)
                                .padding(theme::SM)
                                .width(Length::Fixed(300.0)),
                        ),
                        secondary("Use", Message::UseLocation).enabled(true),
                        secondary("Open folder", Message::OpenLibrary).enabled(true),
                    ]
                    .spacing(theme::SM)
                    .align_y(Alignment::Center),
                ),
            ]));
            body = body.extend(self.hint(
                "Battle.net and the Wine prefix are installed here, and stay here whatever \
                 drives you add later. Games can go on any drive — see Storage.",
            ).map(Element::from));
        }

        // On the Settings page the steps share one card like every other
        // group there; the wizard keeps a row apiece.
        let framed = |row: Element<'static, Message>| -> Element<'static, Message> {
            match in_wizard {
                true => container(row).width(Length::Fill).padding(theme::MD).style(theme::row).into(),
                false => container(row).width(Length::Fill).padding(theme::MD).into(),
            }
        };
        let mut steps: Vec<Element<'_, Message>> = Vec::new();
        let step = |done: bool, label: &'static str, detail: String| -> Element<'static, Message> {
            let color = match done {
                true => theme::OK,
                false => theme::MUTED,
            };
            framed(
                row![
                    container(
                        container(Space::new().width(Length::Fixed(8.0)).height(Length::Fixed(8.0)))
                            .style(theme::dot(color)),
                    )
                    .padding(theme::SM)
                    .style(theme::badge(color)),
                    column![text(label).size(theme::BODY), muted(detail, theme::LABEL)].spacing(theme::XS),
                ]
                .spacing(theme::MD)
                .align_y(Alignment::Center)
                .into(),
            )
        };

        match &self.setup {
            None => body = body.push(muted("checking what is installed…".to_owned(), theme::BODY)),
            Some(status) => {
                // A precondition rather than a step: nothing below can be
                // provisioned without it, and unlike the three steps it is not
                // something Lightforge can do on the user's behalf. It leads
                // the list for the same reason it blocks setup.
                if !status.thirty_two_bit {
                    body = body.push(Self::runtime_row());
                }
                steps.push(step(
                    status.proton.is_some(),
                    "Proton-GE",
                    match &status.proton {
                        Some(build) => format!(
                            "{} — {}",
                            build.version,
                            lightforge_core::setup::describe_source(build)
                        ),
                        None => "not installed".to_owned(),
                    },
                ));
                steps.push(step(
                    status.prefix.is_ready(),
                    "Wine prefix",
                    match status.prefix.is_ready() {
                        true => tilde(&status.prefix.prefix),
                        false => "not built, or the game link is missing".to_owned(),
                    },
                ));
                // Declined at setup is not the same as missing: the user was
                // told what it costs and said no, so the row offers it rather
                // than reporting it as unfinished.
                if !status.battle_net && !status.battle_net_wanted && !in_wizard {
                    steps.push(
                        framed(
                            row![
                                column![
                                    text("Battle.net").size(theme::BODY),
                                    mono("skipped — World of Warcraft only".to_owned()),
                                ]
                                .spacing(theme::XS)
                                .width(Length::Fill),
                                action_button(
                                    "Install it now",
                                    theme::ACCENT,
                                    Message::InstallBattleNet,
                                )
                                .enabled(!self.setup_running),
                            ]
                            .spacing(theme::MD)
                            .align_y(Alignment::Center)
                            .into(),
                        ),
                    );
                } else {
                    steps.push(step(
                        status.battle_net,
                        "Battle.net",
                        match (status.battle_net, status.battle_net_wanted) {
                            (true, _) => tilde(&self.layout.battle_net_dir()),
                            (false, true) => "not installed in the prefix".to_owned(),
                            (false, false) => "skipped — World of Warcraft only".to_owned(),
                        },
                    ));
                }
            }
        }

        body = match in_wizard {
            true => body.extend(steps),
            false if steps.is_empty() => body,
            false => body.push(setting_group(steps)),
        };

        if self.setup_running {
            let mut log = column![].spacing(theme::XS);
            for entry in &self.setup_sample.log {
                log = log.push(muted(entry.clone(), theme::CAPTION));
            }
            log = log.push(match self.setup_sample.fraction() {
                Some(fraction) => Element::from(progress_bar(0.0..=1.0, fraction).girth(6)),
                // A download with no content-length still has to look alive.
                None => Element::from(mono(format!(
                    "{:.0} MB",
                    self.setup_sample.done as f64 / 1e6
                ))),
            });
            body = body.push(container(log).padding(theme::MD).style(theme::card));
        }

        // Where the progress was, so a failed run says why. The Settings page
        // shows its errors above every section; the wizard covers that, and a
        // step that failed used to look like a button that did nothing.
        if in_wizard && !self.setup_running {
            if let Some(e) = &self.error {
                body = body.push(problem(e));
            }
        }

        let ready = self.setup.as_ref().is_some_and(|s| s.is_ready());
        let label = match (self.setup_running, ready) {
            (true, _) => "Working…",
            (false, true) => "Re-run setup",
            (false, false) => "Set up Lightforge",
        };
        let start = action_button(
            label,
            match ready {
                true => theme::PAUSE,
                false => theme::INSTALL,
            },
            Message::RunSetup,
        )
        .width(220.0)
        .enabled(!self.setup_running);
        body = body.push(
            row![start]
                // The same steps, walked through with the questions a first run
                // asks, region, language, drives, and the installs that were
                // here first. Kept here because this is where someone who
                // dismissed it comes looking for it; inside the wizard it would
                // be a button that reopens the thing you are looking at.
                .extend((!in_wizard).then(|| {
                    Element::from(
                        secondary("Run the setup wizard", Message::Wizard(wizard::Msg::Open))
                            .enabled(!self.setup_running),
                    )
                }))
                // Games installed after setup, by Lutris or another Battle.net,
                // without walking through region and drives again.
                .extend((!in_wizard).then(|| {
                    Element::from(
                        secondary("Look for games", Message::Wizard(wizard::Msg::Scan))
                            .enabled(!self.setup_running),
                    )
                }))
                .spacing(theme::SM)
                .align_y(Alignment::Center),
        );

        body.into()
    }

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

    /// Settings: every section on one page, with an index down the left
    /// that jumps to each and marks the one being read.
    ///
    /// It used to be one section at a time, the way Battle.net lays out its
    /// own, and half the sections were a switch or two on an empty page.
    /// Rows stay flat at one shared width rather than a stack of boxes that
    /// each size themselves.
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

        // The error leads rather than trails: a failed setup run is reported
        // here, and at the foot of the page it would go unseen.
        //
        // Every section takes the width it is given. The column used to be
        // capped at a form's measure, which left two thirds of the window
        // empty on a wide monitor and crammed every figure into the left of
        // it; the rows inside already right-align their controls, so the space
        // goes where it is useful rather than being thrown away.
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
    /// Judged the way that product actually starts: a WoW version under its
    /// own pin or the app default, a hand-off title under its pin or
    /// Battle.net's. Only the fallback differs, a pin wins on either path.
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
    /// Installed rather than configured, because the list is otherwise
    /// fourteen rows of which three can launch. Read from Battle.net's own
    /// `product.db`, which is the one place that knows about every install in
    /// the prefix, Lightforge's own and the client's alike.
    pub(crate) fn installed_products(&self) -> Vec<(ProductCode, String)> {
        self.config
            .products
            .iter()
            .filter(|p| self.installed_versions.contains_key(p.code.as_str()))
            .map(|p| (p.code.clone(), p.display_name.clone()))
            .collect()
    }

    /// Which Proton each game runs under, and the update check.
    ///
    /// Every installed game, not the one the Play page is showing: Settings is
    /// about the install as a whole, and a row that renamed itself as you
    /// switched tiles read as a bug, correctly, since nothing on the page
    /// said which game it meant.
    pub(crate) fn proton_section(&self) -> Element<'_, Message> {
        // Built per row rather than once: the first entry is "no pin of my
        // own", and what that falls through to is different on a title
        // Battle.net opens than on a game Lightforge launches itself.
        let builds = |unpinned: Unpinned| -> Vec<ProtonChoice> {
            std::iter::once(ProtonChoice(None, unpinned))
                .chain(
                    self.setup
                        .iter()
                        .flat_map(|s| s.installed.iter())
                        .map(|build| ProtonChoice(Some(build.version.clone()), unpinned)),
                )
                .collect()
        };

        let picker = |key: String,
                      unpinned: Unpinned,
                      current: Option<String>,
                      on_pick: Box<dyn Fn(ProtonChoice) -> Message>| {
            self.picker(
                key,
                builds(unpinned),
                Some(ProtonChoice(current, unpinned)),
                "",
                on_pick,
                Length::Fixed(260.0),
                theme::BODY,
            )
        };

        // A row: the name and what starts it take the slack, then any signs,
        // then the picker, a fixed width so every row's control lines up.
        fn line<'a>(
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

        let games = self.installed_products();
        let mut pins: Vec<Element<'_, Message>> = Vec::new();

        // One row per installed game, in the order the config lists them,
        // the same order as the Play page's tiles, so the two read alike.
        for (code, label) in &games {
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

            pins.push(line(
                label.clone(),
                match Kind::of(code) {
                    Kind::Managed => "Started by Lightforge",
                    Kind::HandOff => "Started by Battle.net",
                },
                tags,
                picker(
                    format!("proton {}", code.as_str()),
                    match Kind::of(code) {
                        Kind::Managed => Unpinned::Default,
                        Kind::HandOff => Unpinned::BattleNet,
                    },
                    pin,
                    Box::new(move |choice| Message::PinProductProton(for_code.clone(), choice)),
                ),
            ));
        }

        // Nothing installed is worth saying: an empty card looks like a page
        // that failed to load.
        if games.is_empty() {
            pins.push(setting("No games yet", "Games appear here once they are installed.", None, Space::new()));
        }

        // A group of their own, because they are not games. The rows above
        // are things you launch; these two are what they fall back to.
        let mut fallbacks: Vec<Element<'_, Message>> = Vec::new();
        fallbacks.push(line(
            "Default".to_owned(),
            "Every game Lightforge starts without a pin of its own.",
            Vec::new(),
            picker(
                "proton default".to_owned(),
                Unpinned::Newest,
                self.config.default_proton.version().map(str::to_owned),
                Box::new(Message::PinDefaultProton),
            ),
        ));
        fallbacks.push(line(
            "Battle.net".to_owned(),
            "Battle.net, Agent, and the games it starts without a pin.",
            // The prefix's own build, beside the pin that will next open it.
            // The one fact that makes the signs above legible.
            vec![match self.setup.as_ref().and_then(|s| s.prefix.built_by.as_deref()) {
                Some(built_by) => muted(format!("prefix built by {built_by}"), theme::CAPTION),
                None => muted("no prefix yet".to_owned(), theme::CAPTION),
            }
            .into()],
            picker(
                "proton battle.net".to_owned(),
                Unpinned::Default,
                self.config.battle_net_proton.version().map(str::to_owned),
                Box::new(Message::PinBattleNetProton),
            ),
        ));

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

        let notes = column![]
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
            );

        // Two questions: which Proton each game runs, and which Protons are
        // on this machine.
        column![
            group_label("Games", None),
            setting_group(pins),
            group_label("When a game has no pin", None),
            setting_group(fallbacks),
            notes,
            group_label("Installed builds", Some(self.proton_install())),
            setting_card(self.proton_builds()),
        ]
        .spacing(theme::SM)
        .into()
    }

    /// Speed caps, ours and Battle.net's, and pausing while playing.
    /// The Addons section: what every addon follows unless it says otherwise.
    pub(crate) fn addons_section(&self) -> Element<'_, Message> {
        let choices: Vec<ChannelChoice> = Channel::ALL.map(ChannelChoice).to_vec();

        setting_group([
            setting(
                "Update channel",
                "The newest kind of build an addon may update to. An addon can choose its own.",
                self.hint(
                    "A channel is a floor, not a filter: allowing alpha builds still installs \
                     the newest release when that is newer. Only CurseForge and GitHub publish \
                     pre-release builds; other sources have one build, so the setting is \
                     inert there.",
                ),
                self.picker(
                    "addon channel".to_owned(),
                    choices,
                    Some(ChannelChoice(self.config.addon_channel)),
                    "",
                    Message::AddonChannelDefault,
                    // The Proton pickers' size, so every dropdown on the page
                    // is one width and one text size.
                    Length::Fixed(260.0),
                    theme::BODY,
                ),
            ),
            setting(
                "Keep addons up to date",
                "Once per game per session, before you play.",
                self.hint(
                    "When Lightforge starts, and again the first time you switch to another \
                     game. A launch waits for it rather than writing into the AddOns folder the \
                     client is reading. An addon can opt out on its own, and an ignored addon \
                     never moves either way.",
                ),
                switch("addon auto update", self.config.addon_auto_update, Message::AddonAutoUpdateDefault),
            ),
        ])
    }

    /// What the launcher does around a launch: the settings that are about
    /// the app itself rather than about a product, a transfer or a prefix.
    pub(crate) fn general_section(&self) -> Element<'_, Message> {
        let shortcut: Option<Element<'_, Message>> = self.steam_shortcut.as_ref().map(|outcome| match outcome {
            Ok(done) => muted(done.clone(), theme::LABEL).into(),
            Err(e) => text(e.clone()).size(theme::LABEL).color(theme::ERROR).into(),
        });

        column![
            setting_group([
                setting(
                    "Minimize when a game starts",
                    "The window waits in the taskbar until you bring it back.",
                    self.hint(
                        "It stays minimized after the game exits: a launcher that raises \
                         itself over whatever you did next is worse than one that waits.",
                    ),
                    switch("minimize on launch", self.config.minimize_on_launch, Message::MinimizeOnLaunch),
                ),
                setting(
                    "Start Battle.net minimized",
                    "When it launches a game for you and can sign in by itself.",
                    self.hint(
                        "Without a saved sign-in it comes up as a normal window whatever \
                         this says, because a sign-in nobody can see is a launch that never \
                         happens. Opening Battle.net from the game panel always shows it.",
                    ),
                    switch("battle.net minimized", self.config.battle_net_minimized, Message::BattleNetMinimized),
                ),
                setting(
                    "Close Battle.net when I press Play",
                    "Off keeps it open beside the game.",
                    self.hint(
                        "Only for Play. Launching through Battle.net, and its own install \
                         and repair flows, start the client afresh whatever this says.",
                    ),
                    switch("close battle.net on play", self.config.close_battle_net_on_play, Message::CloseBattleNetOnPlay),
                ),
            ]),
            setting_group([
                setting(
                    "What's new after an update",
                    "Once per version. The notes are always under Lightforge Updates.",
                    None,
                    switch("release notes", !self.config.patch_notes_muted, Message::ShowReleaseNotes),
                ),
                setting(
                    "Tell me about new releases",
                    "Asks GitHub once at startup. Nothing is downloaded.",
                    self.hint(
                        "Your package manager owns the program, so the notice is a link to \
                         the releases page. DISCLAIMER.md lists this along with everything \
                         else Lightforge contacts.",
                    ),
                    switch("update check", self.config.check_for_updates, Message::CheckLauncherUpdates),
                ),
            ]),
            setting_group([
                setting(
                    "Controller and touch",
                    "Fullscreen, larger controls and button hints, for a Steam Deck or a TV.",
                    self.hint("Setup asks on SteamOS; this changes the answer."),
                    switch("controller look", self.console(), Message::ControllerLook),
                ),
                setting(
                    "Steam library",
                    "A shortcut with Lightforge's artwork, so it starts from Game Mode.",
                    self.hint("Setup adds it on a Steam Deck."),
                    secondary("Add to Steam", Message::AddToSteam).enabled(true),
                ),
            ]),
        ]
        .extend(shortcut)
        .spacing(theme::MD)
        .into()
    }

    /// How Battle.net and every game it launches are started.
    ///
    /// One profile, not one per game: Lightforge only sets the environment of
    /// a client it starts cold, and a running client takes launches over its
    /// own IPC queue, so two games from one session cannot differ (see
    /// `Config::battle_net_tweaks`).
    pub(crate) fn battle_net_section(&self) -> Element<'_, Message> {
        let tweaks = self.tweaks_of(Scope::BattleNet);
        let tools = &self.tools;
        let scope = Scope::BattleNet;

        let rows = vec![
            setting(
                "CPU performance profile",
                match tools.game_performance {
                    Some(_) => "Holds the CPU on its performance profile while a game runs.",
                    None => "Needs CachyOS' game-performance, which is not installed.",
                },
                None,
                self.tool_switch(
                    "bnet game performance",
                    tools.game_performance.is_some(),
                    tweaks.wants_game_performance() && tools.game_performance.is_some(),
                    scope,
                    Tweak::GamePerformance,
                ),
            ),
            setting(
                "MangoHud overlay",
                match tools.mangohud {
                    Some(_) => "Frame rate and frame times drawn over the game.",
                    None => "Needs MangoHud, which is not installed.",
                },
                None,
                self.tool_switch(
                    "bnet mangohud",
                    tools.mangohud.is_some(),
                    tweaks.mangohud && tools.mangohud.is_some(),
                    scope,
                    Tweak::MangoHud,
                ),
            ),
            setting(
                "Wayland backend",
                match tools.wayland {
                    true => "Runs on Proton's Wayland backend instead of XWayland.",
                    false => "Only available in a Wayland session.",
                },
                self.hint(
                    "Newer and less travelled than the X11 path, so it is off until asked for.",
                ),
                self.tool_switch(
                    "bnet wayland",
                    tools.wayland,
                    tweaks.wayland && tools.wayland,
                    scope,
                    Tweak::Wayland,
                ),
            ),
        ];

        column![muted(
            "These apply to Battle.net and every game it starts.".to_owned(),
            theme::LABEL,
        )]
        .extend(
            self.hint(
                "They cannot be set per game, because Lightforge only gets to choose the \
                 environment when it starts the client itself: a client that is already \
                 open is handed the launch over its own message queue.",
            )
            .map(Element::from),
        )
        .push(setting_group(rows))
        // The only way to tune a hand-off game beyond the switches above: it
        // has no client settings for Lightforge to write, so anything else
        // has to arrive as an environment variable.
        .push(self.environment_group(scope))
        .spacing(theme::SM)
        .into()
    }

    pub(crate) fn downloads_section(&self) -> Element<'_, Message> {
        let speed = |input: &str, placeholder: &str, on_input: fn(String) -> Message| -> Element<'_, Message> {
            row![
                clickable(
                    format!("speed {placeholder}"),
                    text_input(placeholder, input)
                        .on_input(on_input)
                        .style(theme::field)
                        .size(theme::BODY)
                        .padding(theme::SM)
                        .width(Length::Fixed(110.0)),
                ),
                muted("MB/s".to_owned(), theme::LABEL),
            ]
            .spacing(theme::SM)
            .align_y(Alignment::Center)
            .into()
        };

        setting_group([
            setting(
                "Download speed limit",
                "For game downloads. Proton builds and addons are not capped.",
                None,
                speed(&self.limit_input, "unlimited", Message::DownloadLimitChanged),
            ),
            setting(
                "Battle.net's speed limit",
                "Battle.net's own cap, for what it downloads.",
                self.hint(
                    "Read from its config and written back; clearing the field removes the \
                     setting and restores its default.",
                ),
                speed(&self.bnet_limit_input, "none", Message::BattleNetLimitChanged),
            ),
            setting(
                "Pause downloads while playing",
                "The game you launch always pauses its own; this pauses the rest of the queue.",
                self.hint(
                    "The game and the launcher would otherwise be sharing one storage file.",
                ),
                switch(
                    "pause while playing",
                    self.config.pause_downloads_while_playing,
                    Message::PauseWhilePlaying,
                ),
            ),
        ])
    }

    /// Where the library lives, how full its disk is, and what is using it.
    pub(crate) fn storage_section(&self) -> Element<'_, Message> {
        let report = self.storage.as_ref();

        // ---- One card per drive.
        let mut drives = column![].spacing(theme::MD);
        match report {
            None => drives = drives.push(muted("measuring…".to_owned(), theme::LABEL)),
            Some(report) => {
                for (index, library) in report.libraries.iter().enumerate() {
                    // The first library on a disk speaks for its free space.
                    let same_disk_as = report.libraries[..index]
                        .iter()
                        .find(|earlier| earlier.mount == library.mount)
                        .map(|earlier| earlier.label.as_str());
                    drives = drives.push(self.drive_card(library, same_disk_as));
                }
                drives = drives.spacing(theme::SM);
                drives = drives.extend(self.hint("The default drive is where a game goes unless you pick another one, and \
                     the folder Battle.net proposes for its own titles. It is only that: \
                     Battle.net itself and the Wine prefix stay where they were installed, \
                     above. Removing a drive takes it off this list and deletes nothing — \
                     add it again and Lightforge finds the games on it.").map(Element::from));
            }
        }

        // ---- Partitions that could become one.
        let offered: Vec<(usize, &library::Partition)> = self
            .partitions
            .iter()
            .enumerate()
            .filter(|(_, partition)| !partition.in_use)
            .collect();
        let add: Element<'_, Message> = match offered.is_empty() {
            true => setting_group([setting(
                "Nothing to add",
                "No other mounted partition Lightforge could write to.",
                None,
                Space::new(),
            )]),
            false => setting_group(offered.into_iter().map(|(index, partition)| {
                let free = match partition.disk {
                    Some(disk) => format!("{} free of {}", size_of(disk.free), size_of(disk.total)),
                    None => "size unknown".to_owned(),
                };
                setting(
                    partition.label.clone(),
                    format!(
                        "{} · {} at {}",
                        partition.fstype,
                        partition.device.display(),
                        partition.point.display()
                    ),
                    None,
                    row![
                        muted(free, theme::LABEL),
                        secondary("Add", Message::AddDrive(index)).enabled(true),
                    ]
                    .spacing(theme::MD)
                    .align_y(Alignment::Center),
                )
            })),
        };
        let add_hint = self.hint(
            "Lightforge creates a \"Lightforge\" folder on the partition and installs \
             games under it. Only mounted partitions you can write to are listed: mount the \
             drive the way you normally do and rescan. A partition is added, not a disk: a \
             drive with two partitions can hold two drives here.",
        );

        // ---- Lightforge's own directories, which follow XDG rather than the
        // drives, so they are listed apart rather than attributed to one.
        let mut usage = column![].spacing(theme::SM);
        match report {
            None => usage = usage.push(muted("measuring…".to_owned(), theme::LABEL)),
            Some(report) => {
                let clearing = self.cache_notice.as_deref() == Some("clearing…");
                usage = usage.push(setting_group(report.app.iter().map(|item| {
                    let action = item.clearable.then(|| {
                        secondary("Clear", Message::ClearCache).enabled(item.bytes > 0 && !clearing)
                    });
                    setting(
                        item.label.clone(),
                        tilde(&item.path),
                        None,
                        // The button's slot is kept on every row, so the
                        // sizes line up whether or not there is one.
                        row![
                            text(size_of(item.bytes)).size(theme::BODY).font(theme::MONO),
                            container(action.unwrap_or_else(|| Space::new().into()))
                                .width(Length::Fixed(ACTION_COLUMN))
                                .align_x(Alignment::End),
                        ]
                        .spacing(theme::MD)
                        .align_y(Alignment::Center),
                    )
                })));
                usage = usage.extend(self.hint("Clearing the cache removes downloaded art, installers and lists; each is \
                     fetched again when needed. Shader caches are kept: rebuilding them is \
                     what makes a game stutter.").map(Element::from));
                if let Some(notice) = &self.cache_notice {
                    usage = usage.push(muted(notice.clone(), theme::LABEL));
                }
                if self.storage_measuring {
                    usage = usage.push(muted("updating…".to_owned(), theme::CAPTION));
                }
            }
        }

        column![
            group_label("Drives", None),
            drives,
            group_label(
                "Add a drive",
                Some(secondary("Rescan drives", Message::RefreshDrives).enabled(true)),
            ),
            add,
        ]
        .extend(add_hint.map(Element::from))
        .push(group_label("Lightforge's own files", None))
        .push(usage)
        .spacing(theme::SM)
        .into()
    }

    /// One drive's card: how full it is, what Lightforge put there, and the
    /// three things you can do to it.
    pub(crate) fn drive_card<'a>(
        &'a self,
        library: &'a storage::LibraryUsage,
        same_disk_as: Option<&'a str>,
    ) -> Element<'a, Message> {
        let id = library.id.clone();
        let mut card = column![].spacing(theme::SM);

        let mut title = row![text(library.label.clone()).size(theme::HEADING)]
            .spacing(theme::SM)
            .align_y(Alignment::Center);
        if library.is_default {
            title = title.push(
                container(text("Default").size(theme::CAPTION).color(theme::ACCENT))
                    .padding([2.0, theme::SM])
                    .style(theme::tag(theme::ACCENT)),
            );
        }
        if let Some(reason) = library.state.reason(&library.label) {
            title = title.push(
                container(text(reason).size(theme::CAPTION).color(theme::ERROR))
                    .padding([2.0, theme::SM])
                    .style(theme::tag(theme::ERROR)),
            );
        }
        title = title.push(Space::new().width(Length::Fill));
        title = title.push(mono(format!(
            "{} → games   {} → wow",
            library.letters.0.to_uppercase(),
            library.letters.1.to_uppercase(),
        )));
        card = card.push(title);
        card = card.push(muted(tilde(&library.path), theme::LABEL));

        match library.disk {
            // Already drawn on the card above: one bar per disk.
            Some(_) if same_disk_as.is_some() => {
                card = card.push(muted(
                    format!(
                        "Same disk as {} — {} is Lightforge's here",
                        same_disk_as.unwrap_or_default(),
                        size_of(library.total()),
                    ),
                    theme::CAPTION,
                ));
            }
            Some(disk) => {
                let used = disk.total.saturating_sub(disk.free);
                let fraction = match disk.total {
                    0 => 0.0,
                    total => used as f32 / total as f32,
                };
                card = card
                    .push(progress_bar(0.0..=1.0, fraction).girth(6))
                    .push(muted(
                        format!(
                            "{} free of {} — {} is Lightforge's",
                            size_of(disk.free),
                            size_of(disk.total),
                            size_of(library.total()),
                        ),
                        theme::CAPTION,
                    ));
            }
            // A drive that is not here is not measured at all: walking its
            // mount point would report whatever filesystem is *under* it as
            // if the library's games were on it.
            None => {
                card = card.push(muted(
                    "Nothing is measured while the drive is away. The games on it are \
                     untouched, and come back with it."
                        .to_owned(),
                    theme::CAPTION,
                ))
            }
        }
        // Only where it is actually true: a library can be unclaimed and still
        // hold games, which is every install from before multi-drive storage,
        // and the drive holding the prefix always has Battle.net's own titles
        // on it.
        if library.state == library::State::New && library.total() == 0 {
            card = card.push(muted(
                "Nothing installed here yet. The folder is created with the first game.".to_owned(),
                theme::CAPTION,
            ));
        }

        for item in &library.usage {
            // Nothing is a fact better told by absence than by a row of
            // zeroes with a dead button beside it.
            if item.bytes == 0 {
                continue;
            }
            let notice = self
                .drive_notice
                .as_ref()
                .filter(|(which, _)| *which == library.id)
                .map(|(_, note)| note.clone());
            let clearing = notice.as_deref() == Some("clearing…");
            let action = item.clearable.then(|| {
                secondary("Clear", Message::ClearDrive(id.clone()))
                    .enabled(item.bytes > 0 && !clearing)
            });
            card = card.push(usage_row(
                text(item.label.clone()).size(theme::BODY).into(),
                item.bytes,
                action,
            ));
        }
        if let Some((which, note)) = &self.drive_notice {
            if *which == library.id {
                card = card.push(muted(note.clone(), theme::LABEL));
            }
        }
        card = card.extend(self.unused_rows(library));

        let mut actions = row![Space::new().width(Length::Fill)]
            .spacing(theme::SM)
            .align_y(Alignment::Center);
        // `is_present` rather than `is_ready`: a drive with nothing of ours on
        // it yet can still be opened and can still be made the default, it is
        // set up by the first install, not before. Only a drive that is *away*
        // can do neither.
        actions = actions.push(
            secondary("Open folder", Message::OpenDrive(id.clone()))
                .enabled(library.state.is_present()),
        );
        actions = actions.push(
            secondary("Make default", Message::SetDefaultDrive(id.clone()))
                .enabled(!library.is_default && library.state.is_present()),
        );
        // The default drive cannot be removed while another exists, make a
        // different one the default first, and the last one cannot be
        // removed at all, since every path resolves through it.
        actions = actions.push(
            secondary("Remove", Message::RemoveDrive(id.clone()))
                .enabled(!library.is_default && self.config.libraries.len() > 1),
        );
        card = card.push(actions);

        setting_card(card)
    }

    /// A drive card's "Unused game data" row and the line under it.
    pub(crate) fn unused_rows<'a>(&'a self, library: &'a storage::LibraryUsage) -> Vec<Element<'a, Message>> {
        let label = || text("Unused game data").size(theme::BODY).into();
        let Some(state) = self.unused.get(&library.id) else {
            return Vec::new();
        };
        match state {
            UnusedData::Checking => vec![muted("checking for unused game data…".to_owned(), theme::CAPTION).into()],
            // Nothing worth a button: told by absence, like an empty cache.
            UnusedData::Ready(plan) if plan.freeable < UNUSED_WORTH_SHOWING => Vec::new(),
            UnusedData::Ready(plan) => {
                let busy = self
                    .config
                    .libraries
                    .iter()
                    .find(|l| l.id == library.id)
                    .is_some_and(|l| self.downloading_into(&self.layout.wow_install_in(l)));
                let held = self.store_writer().is_some() || self.launching.is_some();
                let action =
                    secondary("Free up", Message::FreeUnused(library.id.clone())).enabled(!busy && !held);
                let note = match busy {
                    true => "Finish or cancel the download on this drive first.".to_owned(),
                    false if plan.moved > 0 => format!(
                        "Left behind by patches and versions that are gone. {} of game data \
                         still in use is copied first. The game and Battle.net have to be closed.",
                        size_of(plan.moved)
                    ),
                    false => "Left behind by patches and versions that are gone. The game and \
                              Battle.net have to be closed."
                        .to_owned(),
                };
                vec![
                    usage_row(label(), plan.freeable, Some(action)),
                    muted(note, theme::CAPTION).into(),
                ]
            }
            UnusedData::Freeing { copied, total } => {
                let done = copied.load(std::sync::atomic::Ordering::Relaxed).min(*total);
                vec![muted(
                    format!("freeing unused game data… {} of {} copied", size_of(done), size_of(*total)),
                    theme::LABEL,
                )
                .into()]
            }
            UnusedData::Said(note) => vec![muted(note.clone(), theme::LABEL).into()],
        }
    }

    /// The version, and where its configuration lives.
    pub(crate) fn about_section(&self) -> Element<'_, Message> {
        let (status, asking) = match &self.launcher_check {
            LauncherCheck::Idle => (String::new(), false),
            LauncherCheck::Asking => ("Asking GitHub.".to_owned(), true),
            LauncherCheck::Newest => ("This is the newest version.".to_owned(), false),
            LauncherCheck::Failed(why) => (format!("Could not check: {why}"), false),
        };
        let mut rows = vec![
            setting(
                "Version",
                status,
                None,
                row![
                    text(env!("CARGO_PKG_VERSION")).size(theme::BODY).font(theme::MONO),
                    secondary(if asking { "Checking" } else { "Check for updates" }, Message::CheckLauncherNow)
                        .enabled(!asking),
                ]
                .spacing(theme::MD)
                .align_y(Alignment::Center),
            ),
        ];
        if let Some(dirs) = &self.dirs {
            rows.push(setting(
                "Settings file",
                tilde(&dirs.config_file()),
                None,
                secondary("Open folder", Message::OpenConfigFolder).enabled(true),
            ));
        }
        setting_group(rows)
    }
}

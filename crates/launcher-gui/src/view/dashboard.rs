//! The Play page: art and the action panel.

use crate::*;
use iced::widget::column;

impl Lightforge {
    pub(crate) fn handoff_panel(&self) -> Element<'_, Message> {
        let mut left = column![].spacing(theme::SM).width(Length::Fill);
        left = left.push(match &self.error {
            Some(e) => problem(e),
            None => text(self.status.clone()).size(theme::LABEL).color(theme::MUTED).into(),
        });

        left = self.scan_progress(left);
        match self.installed_versions.get(self.selected.code.as_str()) {
            Some(version) => {
                left = left.push(
                    row![muted("installed".to_owned(), theme::LABEL), mono(version.clone())]
                        .spacing(theme::SM)
                        .align_y(Alignment::Center),
                );
            }
            // Said out loud, because the button changing word for word is
            // easy to miss on a screen that is mostly key art.
            None => left = left.push(muted("not installed".to_owned(), theme::LABEL)),
        }

        let act = self.handoff_act();
        left = left.extend(self.hint(act.hint()).map(Element::from));
        // The managed panel has a row of links and this one had none, so a
        // hand-off game had no route to any launch settings. It is phrased
        // for the shared profile on purpose, "its games", not "this game",
        // because one environment covers every game the client starts.
        left = left.push(self.ring(
            focus::Target::HandOffSettings,
            Some(Message::ShowSettingsTab(SettingsTab::BattleNet)),
            button(
                row![
                    image(sliders_icon())
                        .width(Length::Fixed(LINK_GLYPH))
                        .height(Length::Fixed(LINK_GLYPH)),
                    text("How Battle.net and its games launch").size(theme::LABEL),
                ]
                .spacing(theme::XS)
                .align_y(Alignment::Center),
            )
            .on_press(Message::ShowSettingsTab(SettingsTab::BattleNet))
            .style(theme::ghost)
            .padding(theme::tap([theme::XS, theme::SM])),
        ));
        let stoppable = self.busy && self.game_up && !self.stopping;
        let open = button(
            container(text(match self.busy && self.game_up {
                true => self.stop_label(),
                false => act.label(),
            })
            .size(theme::HEADING))
            .center_x(Length::Fill),
        )
        // Dressed as the Play control on a game Lightforge runs itself,
        // same 240 by CONTROL_HEIGHT, same heading, same filled face, because
        // it is the same act. The hand-off drives Battle.net end to end now,
        // so a quiet outlined button would be understating what it does.
        //
        // Blue rather than PLAY's gold: it starts through Battle.net, and
        // the colour is the one thing that can say so without a second line
        // of text. When there is no game family to ask for, the button only
        // opens the client, not a launch, so it does not dress as one.
        .style(move |base: &iced::Theme, status| match (stoppable, act.colour()) {
            (true, _) => theme::filled(theme::ERROR)(base, status),
            (_, Some(colour)) => theme::filled(colour)(base, status),
            (_, None) => theme::outlined(base, status),
        })
        .padding([theme::MD, theme::LG])
        .width(Length::Fixed(240.0))
        .height(Length::Fixed(theme::CONTROL_HEIGHT));

        let press = match stoppable {
            true => Message::AskStopGame,
            false => act.message(),
        };
        let can_press = stoppable || !self.busy;

        container(
            row![
                left,
                self.ring(
                    focus::Target::Primary,
                    can_press.then(|| press.clone()),
                    match can_press {
                        false => open,
                        true => open.on_press(press),
                    },
                ),
            ]
            .spacing(theme::XL)
            .align_y(Alignment::Center),
        )
        .padding(theme::LG)
        .style(theme::floating)
        .into()
    }

    /// A screen that is not the dashboard: header, title, content.
    /// A page with a title, optionally something at the far end of that line,
    /// and a scrolling body.
    ///
    /// `aside` is for a fact about the page rather than a control, a count,
    /// a state. It rides on the title's line because that is where the eye
    /// already is, and because a figure like that under the title reads as a
    /// subtitle explaining the page, which it is not.
    pub(crate) fn page<'a>(
        &'a self,
        title: &'a str,
        subtitle: Option<String>,
        aside: Option<Element<'a, Message>>,
        content: Element<'a, Message>,
    ) -> Element<'a, Message> {
        let mut heading = column![text(title).size(theme::TITLE)].spacing(theme::XS);
        if let Some(subtitle) = subtitle {
            heading = heading.push(muted(subtitle, theme::LABEL));
        }

        let mut top = row![heading].align_y(Alignment::Center);
        if let Some(aside) = aside {
            top = top.push(Space::new().width(Length::Fill)).push(aside);
        }

        column![
            self.header(),
            column![
                top,
                focus::region(
                    "page",
                    scrollable(content)
                        .direction(scrollable::Direction::Vertical(theme::scrollbar_shape()))
                        .style(theme::scrollbar)
                        .height(Length::Fill),
                ),
            ]
            .spacing(theme::LG)
            .padding(theme::LG),
        ]
        .into()
    }

    pub(crate) fn dashboard(&self) -> Element<'_, Message> {
        let action = self.action();
        let kind = self.kind_of(&self.selected.code);

        // A hand-off wears art only when the manifest names it *itself*.
        //
        // The `default` entry catches every code the manifest does not name,
        // which is deliberate for a WoW product, a line released after this
        // binary should look like WoW rather than like nothing, and wrong for
        // a hand-off, where it would dress Hearthstone in Retail's logo and
        // key art. That was the whole reason hand-offs showed no art at all.
        //
        // Now that each one carries its own logo, the rule can be the precise
        // one instead of the blunt one: inherit if you are WoW, otherwise show
        // art only if it is yours. A Blizzard game that appears in the catalog
        // before its art is published still gets nothing, which is correct.
        let themed_itself = self.manifest.as_ref().is_some_and(|manifest| {
            manifest
                .own_theme(&self.selected.code)
                .is_some_and(|theme| theme.logo.is_some())
        });
        let branding = match kind {
            Kind::Managed => self.branding.as_ref(),
            Kind::HandOff => self.branding.as_ref().filter(|_| themed_itself),
        };

        // The wordmark hangs under the app title in the top-left rather than
        // floating mid-page: the centre of the dashboard belongs to the art,
        // and a product logo reads as branding when it sits with Lightforge's
        // own rather than competing with the background for the middle.
        let logo: Element<Message> = match branding {
            // Equal area per mark (see [`theme::logo_size`]) in a box that
            // never moves: centred down it and near its left edge, with
            // [`theme::logo_offset`] nudging the narrowest marks in so they
            // do not hang further out than the rest.
            Some(b) => {
                let (width, height) = theme::logo_size(b.logo_aspect, b.logo_ink);
                container(
                    image(b.logo.clone())
                        .content_fit(ContentFit::Contain)
                        .width(Length::Fixed(width))
                        .height(Length::Fixed(height)),
                )
                .align_x(Alignment::Start)
                .center_y(Length::Fixed(theme::LOGO_BOX_HEIGHT))
                .width(Length::Fixed(theme::LOGO_BOX_WIDTH))
                .padding(iced::Padding {
                    left: theme::logo_offset(width),
                    ..iced::Padding::ZERO
                })
                .into()
            }
            None => column![
                text(self.selected.label.clone()).size(theme::DISPLAY),
                muted("no artwork configured".to_owned(), theme::LABEL),
            ]
            .spacing(theme::XS)
            .into(),
        };

        let content = column![
            self.header(),
            column![
                // The wordmark hangs straight under the band (a gap leaves it
                // floating in the art) and is inset by [`theme::LOGO_INSET`],
                // since a mark trimmed to its ink would otherwise touch the
                // page edge.
                row![
                    column![
                        container(logo)
                            .align_x(Alignment::Start)
                            .width(Length::Fill)
                            .padding(iced::Padding {
                                top: 0.0,
                                right: 0.0,
                                bottom: theme::MD,
                                left: theme::LOGO_INSET,
                            }),
                        Space::new().height(Length::Fill),
                    ]
                    .width(Length::Fill),
                ]
                .spacing(theme::MD)
                .height(Length::Fill),
                // A product Battle.net owns gets a panel that offers only what
                // Lightforge can actually do for it. Running the normal one
                // would put an INSTALL button on a game this launcher has no
                // way to install, and, worse, one that would plan a CASC
                // download into the WoW directory.
                match kind {
                    Kind::Managed => self.action_panel(&action),
                    Kind::HandOff => self.handoff_panel(),
                },
            ]
            .spacing(theme::MD)
            // No inset at all at the top, where the other three sides take
            // the page's: that inset is right for the action panel against
            // the window edge and wrong for a wordmark, which should read as
            // hanging from the band directly above it. The logo box carries
            // its own slack anyway, the mark is centred in it, so the gap
            // under the band is real without being spent here.
            .padding(iced::Padding {
                top: 0.0,
                right: theme::LG,
                bottom: theme::LG,
                left: theme::LG,
            }),
        ];

        match branding {
            Some(b) => stack![
                image(b.background.clone())
                    .content_fit(ContentFit::Cover)
                    .width(Length::Fill)
                    .height(Length::Fill),
                // A scrim at both ends: the controls at the bottom and the
                // navigation row at the top sit straight on the art. Offsets
                // run bottom (0.0) to top (1.0). The top pair holds rather than
                // ramps, so the nav row (about 6% down) sits on full cover
                // instead of mid-fade; the fade lands over the game band, which
                // has its own card.
                container(Space::new().height(Length::Fill))
                    .width(Length::Fill)
                    .style(|_| container::Style {
                        background: Some(iced::Background::Gradient(iced::Gradient::Linear(
                            iced::gradient::Linear::new(0.0)
                                .add_stop(0.0, iced::Color { a: 0.95, ..theme::BG })
                                .add_stop(0.45, iced::Color { a: 0.35, ..theme::BG })
                                .add_stop(0.84, iced::Color { a: 0.12, ..theme::BG })
                                .add_stop(0.93, iced::Color { a: 0.75, ..theme::BG })
                                .add_stop(1.0, iced::Color { a: 0.75, ..theme::BG }),
                        ))),
                        ..container::Style::default()
                    }),
                content,
            ]
            .into(),
            None => content.into(),
        }
    }

    /// The word on the button while a game is up.
    fn stop_label(&self) -> &'static str {
        match self.stopping {
            true => "STOPPING…",
            false => "STOP",
        }
    }

    /// Status, progress, and every action for the selected product.
    pub(crate) fn action_panel(&self, action: &ActionState) -> Element<'_, Message> {
        let color = match action {
            ActionState::Install => theme::INSTALL,
            ActionState::Update { .. } => theme::UPDATE,
            ActionState::Play => theme::PLAY,
            ActionState::Downloading(_) | ActionState::Paused(_) | ActionState::Queued => {
                theme::PAUSE
            }
            ActionState::Error { .. } => theme::ERROR,
            ActionState::Unknown | ActionState::Offline { .. } => theme::PAUSE,
        };

        let mut left = column![].spacing(theme::SM).width(Length::Fill);

        // The branch belongs here rather than in the header: it is not
        // navigation, it decides what every line below it refers to, which
        // version is "latest", which install "installed", which files "Open
        // Files" opens. Only shown where there is a choice to make.
        if let Some((branches, current)) = self.branch_choices() {
            left = left.push(
                row![
                    muted("version".to_owned(), theme::CAPTION),
                    self.picker(
                        "version".to_owned(),
                        branches,
                        Some(current),
                        "",
                        Message::BranchSelected,
                        Length::Shrink,
                        theme::LABEL,
                    ),
                ]
                .spacing(theme::SM)
                .align_y(Alignment::Center),
            );
        }

        left = left.push(match (&self.error, action) {
            (Some(e), _) => problem(e),
            // The drive outranks whatever the last refresh had to say: a
            // "latest 11.2.7" under a game that cannot be started is the
            // wrong fact to be reading.
            (None, ActionState::Offline { drive }) => text(format!(
                "{drive} is not mounted — the game is still on it, untouched"
            ))
            .size(theme::LABEL)
            .color(theme::ERROR)
            .into(),
            (None, _) => text(self.status.clone()).size(theme::LABEL).color(theme::MUTED).into(),
        });

        left = self.scan_progress(left);

        if let Some(progress) = progress_of(action) {
            // Two jobs for one bar. While bytes are moving it reports them.
            // Before the total is known, and again after the last byte lands
            // while the install is still extracting and writing `.build.info`,
            // there is no fraction to report but there is still work going on,
            // so the bar sweeps instead of sitting at a number that would be
            // a lie in one direction or the other.
            let sweeping = progress.total_bytes == 0
                || progress.downloaded_bytes >= progress.total_bytes;
            let value = match sweeping {
                true => sweep(),
                false => progress.fraction(),
            };
            left = left.push(progress_bar(0.0..=1.0, value).girth(6));
            left = left.push(
                row![
                    mono(bytes_of(progress.downloaded_bytes, progress.total_bytes)),
                    dot(),
                    mono(rate_of(progress.bytes_per_second)),
                    dot(),
                    mono(eta_of(progress)),
                ]
                .spacing(theme::SM)
                .align_y(Alignment::Center),
            );
            let stage = self
                .download_for(&self.selected.code)
                .map(|d| d.sample.stage.clone())
                .unwrap_or_default();
            if !stage.is_empty() {
                left = left.push(muted(stage, theme::CAPTION));
            }
        } else if let Some(version) = &self.local.version {
            left = left.push(
                row![muted("installed".to_owned(), theme::CAPTION), mono(version.clone())]
                    .spacing(theme::SM)
                    .align_y(Alignment::Center),
            );
        }

        // What an Install or Update would actually fetch beyond the base game.
        // The size surprise this prevents is the whole reason the gate exists:
        // on the Forever beta the texture pack alone is 37 GiB.
        let opted = self.opted_optional(&self.selected.code);
        if !opted.is_empty() && matches!(action, ActionState::Install | ActionState::Update { .. })
        {
            let names: Vec<String> = opted.iter().map(|t| optional_label(t)).collect();
            left = left.push(muted(
                format!("includes optional content: {}", names.join(", ")),
                theme::CAPTION,
            ));
        }

        // Three words in a row with nothing on them read as a caption, not as
        // somewhere to go. A mark in front of each says "this is a control"
        // before the word is read at all.
        //
        // Padded on all four sides, not just vertically: the hover fill is the
        // button's own bounds, so with no horizontal padding the glyph and the
        // last letter sit flush against the rounded edge and the label reads as
        // if it were spilling out of the box. The cost is that the group starts
        // one [`theme::SM`] in from "version" and "installed" above it, which
        // is a smaller wrong than text touching its own border.
        let link = |mark: image::Handle, label: &'static str, message: Option<Message>| {
            button(
                row![
                    image(mark)
                        .width(Length::Fixed(LINK_GLYPH))
                        .height(Length::Fixed(LINK_GLYPH))
                        .opacity(match message.is_some() {
                            true => 1.0_f32,
                            false => DIMMED,
                        }),
                    text(label).size(theme::tap_text(theme::LABEL)),
                ]
                .spacing(theme::XS)
                .align_y(Alignment::Center),
            )
            .on_press_maybe(message)
            .style(theme::ghost)
            .padding(theme::tap([theme::XS, theme::SM]))
        };

        left = left.push(
            row![
                // Addons are already per-product, the screen titles itself
                // with the selection and every task takes its code, so this
                // is navigation, not a second copy of it. Disabled until
                // there is an install to put an addon into.
                self.ring(
                    focus::Target::Addons,
                    self.local.version.is_some().then_some(Message::Show(Screen::Addons)),
                    link(
                        package_icon(),
                        "Manage Addons",
                        self.local.version.is_some().then_some(Message::Show(Screen::Addons)),
                    ),
                ),
                self.ring(
                    focus::Target::GameSettings,
                    Some(Message::OpenGameSettings),
                    link(sliders_icon(), "Game Settings", Some(Message::OpenGameSettings)),
                ),
                self.ring(
                    focus::Target::Subscription,
                    Some(Message::ManageSubscription),
                    link(card_icon(), "Manage Subscription", Some(Message::ManageSubscription)),
                ),
            ]
            // Each link now carries its own air, so the row's gap only has to
            // keep two hover fills from touching; [`theme::MD`] on top of that
            // padding would read as three separate groups.
            .spacing(theme::XS)
            .align_y(Alignment::Center),
        );

        // Battle.net's own words for a game that downloads before it opens.
        // A caption and a label only: PLAY stays PLAY, because whether the
        // game is open is decided on Blizzard's side, and this cache can
        // still say "pre-install" on release day.
        let release_date = self.families.release_date(self.selected.code.as_str(), "default");
        let label = match (action, release_date) {
            _ if self.busy && self.game_up => self.stop_label(),
            (ActionState::Install, Some(_)) => "PRE-INSTALL",
            _ => action.label(),
        };
        // A game that is up turns PLAY into STOP, in the colour of the other
        // thing this button never is: an error.
        let stoppable = self.busy && self.game_up && !self.stopping;
        let color = if self.busy && self.game_up { theme::ERROR } else { color };
        let primary = button(
            container(text(label).size(theme::HEADING).color(iced::Color::WHITE))
                .center_x(Length::Fill),
        )
        .style(theme::filled_left(color))
        .padding([theme::MD, theme::LG])
        // 190 + the tab's 50 is 240: the width of the Battle.net button
        // underneath, so the two stack flush rather than nearly flush.
        .width(Length::Fixed(190.0))
        .height(Length::Fixed(theme::CONTROL_HEIGHT));
        let battle_net = button(
            container(text("Launch via Battle.net").size(theme::tap_text(theme::LABEL))).center_x(Length::Fill),
        )
        .style(theme::outlined)
        .padding(theme::tap([theme::SM, theme::MD]))
        .width(Length::Fixed(240.0));

        // A split button, welded to the primary action rather than floating
        // beside it: the cog is a second way into the same thing, which is how
        // Battle.net builds it. Zero spacing and complementary corner radii
        // are what make the pair read as one control.
        //
        // The icon is an image, not a glyph, the UI font has no U+2699 and
        // renders it as a tofu box.
        let gear = button(
            image(gear_icon())
                .width(Length::Fixed(18.0))
                .height(Length::Fixed(18.0)),
        )
        .style(theme::filled_right(color))
        .padding([theme::MD, theme::MD])
        // The same fixed height as the action half, never `Fill`: inside a
        // column, `Fill` resolves against the *column*, and the tab grows into
        // a green stripe the height of the whole stack.
        .height(Length::Fixed(theme::CONTROL_HEIGHT))
        .on_press(Message::ToggleGear);

        let primary_press = match () {
            _ if stoppable => Some(Message::AskStopGame),
            _ if self.busy || matches!(action, ActionState::Unknown) => None,
            _ => Some(Message::Primary),
        };

        // Pinned to the control rather than placed near it. Everything else
        // was layout, and layout cannot put a child over its sibling: in the
        // panel's column the menu sized the card, and above the card its top
        // edge fell between the menu and the button.
        let control = popover::popover(
            row![
                self.ring(
                    focus::Target::Primary,
                    primary_press.clone(),
                    match primary_press {
                        Some(message) => primary.on_press(message),
                        None => primary,
                    },
                ),
                self.ring(focus::Target::Gear, Some(Message::ToggleGear), gear),
            ]
            .align_y(Alignment::Center),
            self.gear_open.then(|| self.gear_menu()),
        );

        let right = column![
            control,
            self.ring(
                focus::Target::LaunchViaBattleNet,
                (!self.busy).then_some(Message::BattleNet),
                match self.busy {
                    true => battle_net,
                    false => battle_net.on_press(Message::BattleNet),
                },
            ),
        ]
        .push(release_date.map(|date| muted(servers_open(date), theme::CAPTION)))
        .spacing(theme::SM)
        .align_x(Alignment::Center)
        .width(Length::Shrink);

        container(row![left, right].spacing(theme::XL).align_y(Alignment::Center))
            .padding(theme::LG)
            .style(theme::floating)
            .into()
    }
}

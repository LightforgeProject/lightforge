//! The Play page: art and the action panel.

use crate::*;
use iced::widget::column;

impl Lightforge {
    /// The action panel for a game Battle.net installs and starts.
    ///
    /// Bare: no Install, no Update, no Play, not even greyed out.
    /// Offering those disabled would say Lightforge could manage this game and
    /// merely will not.
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
        // "its games": one environment covers every game the client starts.
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
        // Dressed as Play, in blue: it starts through Battle.net. Without a
        // game family it only opens the client, and is not dressed as a launch.
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

    /// A page with a title, optionally something at the far end of that line,
    /// and a scrolling body.
    ///
    /// `aside` is a fact (a count, a state), not a control.
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

        // A hand-off wears art only when the manifest names it itself: the
        // `default` fallback would dress Hearthstone in Retail's art.
        let themed_itself = self.manifest.as_ref().is_some_and(|manifest| {
            manifest
                .own_theme(&self.selected.code)
                .is_some_and(|theme| theme.logo.is_some())
        });
        let branding = match kind {
            Kind::Managed => self.branding.as_ref(),
            Kind::HandOff => self.branding.as_ref().filter(|_| themed_itself),
        };

        // The wordmark hangs top-left; the centre belongs to the art.
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
                // Straight under the band, inset by [`theme::LOGO_INSET`].
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
                // Battle.net's games get the hand-off panel, never INSTALL.
                match kind {
                    Kind::Managed => self.action_panel(&action),
                    Kind::HandOff => self.handoff_panel(),
                },
            ]
            .spacing(theme::MD)
            // No top inset: the wordmark hangs from the band.
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
                // A scrim at both ends, bottom 0.0 to top 1.0. The top pair
                // holds, so the nav row sits on full cover.
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
        container(
            row![self.action_details(action), self.action_buttons(action)]
                .spacing(theme::XL)
                .align_y(Alignment::Center),
        )
        .padding(theme::LG)
        .style(theme::floating)
        .into()
    }

    /// The left of the panel: branch, status, progress or the installed
    /// version, optional content, and the links under them.
    fn action_details(&self, action: &ActionState) -> Element<'_, Message> {
        let mut left = column![].spacing(theme::SM).width(Length::Fill);

        // Here, not in the header: every line below refers to the branch.
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
            // The missing drive outranks the version line.
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
            // Sweeps while there is work but no fraction: before the total,
            // and while extracting after the last byte.
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

        // What would come beyond the base game (37 GiB of textures on Forever).
        let opted = self.opted_optional(&self.selected.code);
        if !opted.is_empty() && matches!(action, ActionState::Install | ActionState::Update { .. })
        {
            let names: Vec<String> = opted.iter().map(|t| optional_label(t)).collect();
            left = left.push(muted(
                format!("includes optional content: {}", names.join(", ")),
                theme::CAPTION,
            ));
        }

        left.push(self.product_links()).into()
    }

    /// Manage Addons, Game Settings, Manage Subscription.
    fn product_links(&self) -> Element<'_, Message> {
        // A mark before each says "control". Padded on all sides, or the
        // text touches the hover fill's edge.
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
        row![
            // Disabled until there is an install.
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
        // Each link carries its own air.
        .spacing(theme::XS)
        .align_y(Alignment::Center)
        .into()
    }

    /// The right of the panel: the primary action with its cog, Battle.net
    /// under it, and the release date when there is one.
    fn action_buttons(&self, action: &ActionState) -> Element<'_, Message> {
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

        // Battle.net's pre-install wording, as a caption only: PLAY stays PLAY.
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
        // underneath, so the two stack flush, not nearly flush.
        .width(Length::Fixed(190.0))
        .height(Length::Fixed(theme::CONTROL_HEIGHT));
        let battle_net = button(
            container(text("Launch via Battle.net").size(theme::tap_text(theme::LABEL))).center_x(Length::Fill),
        )
        .style(theme::outlined)
        .padding(theme::tap([theme::SM, theme::MD]))
        .width(Length::Fixed(240.0));

        // A split button, as Battle.net's. The cog is an image: the font has no U+2699.
        let gear = button(
            image(gear_icon())
                .width(Length::Fixed(18.0))
                .height(Length::Fixed(18.0)),
        )
        .style(theme::filled_right(color))
        .padding([theme::MD, theme::MD])
        // Fixed, never `Fill`, which resolves against the whole column.
        .height(Length::Fixed(theme::CONTROL_HEIGHT))
        .on_press(Message::ToggleGear);

        let primary_press = match () {
            _ if stoppable => Some(Message::AskStopGame),
            _ if self.busy || matches!(action, ActionState::Unknown) => None,
            _ => Some(Message::Primary),
        };

        // A popover: layout cannot put a child over its sibling.
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

        column![
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
        .width(Length::Shrink)
        .into()
    }
}

//! Game Optimizations: presets, the four cards and environment variables.

use crate::*;
use iced::widget::column;

impl Lightforge {
    /// The Game Optimizations page: a preset row over a two-column grid.
    ///
    /// The cards are grouped by what someone is trying to do rather than by
    /// the mechanism that applies a switch (host binary, DXVK, `Config.wtf`,
    /// environment), which made six cards of wildly different heights. Four
    /// cards of one size, two to a row, read as a grid.
    pub(crate) fn tweaks_screen(&self) -> Element<'_, Message> {
        let tweaks = self.tweaks();

        // Four cards on one grid, every box the same size whatever is in it.
        // The row carries the height and the cards fill it, which is the only
        // arrangement that gives a uniform box: sized to their content they
        // end at four different places, and iced cannot stretch two fluid
        // siblings against each other with nothing to measure (see
        // [`Lightforge::tweak_card`]).
        let top = Length::Fixed(self.card_height(CardRow::Top, &tweaks));
        let bottom = Length::Fixed(self.card_height(CardRow::Bottom, &tweaks));
        let body = column![
            self.preset_row(&tweaks),
            row![self.performance_card(&tweaks), self.looks_card(&tweaks)]
                .spacing(theme::MD)
                .height(top),
            row![self.input_card(&tweaks), self.advanced_card(&tweaks)]
                .spacing(theme::MD)
                .height(bottom),
            self.env_card(Scope::Product, &tweaks),
        ]
        .spacing(theme::MD);

        // The one sentence that used to be a paragraph inside the biggest
        // card. It is true of every switch above, so it belongs under all of
        // them rather than inside one.
        let mut body = body.extend(
            self.hint(
                "The client settings here are written to this product's Config.wtf and \
                 re-applied before every launch, because the client rewrites that file from \
                 memory when it exits. Change one of them in-game and the setting here wins \
                 next time.",
            )
            .map(Element::from),
        );
        if let Some(e) = &self.error {
            body = body.push(problem(e));
        }

        self.page(
            "Game Optimizations",
            Some(format!("{} · applied when this product launches", self.selected.label)),
            None,
            body.into(),
        )
    }

    /// A checkbox in one of the cards below.
    pub(crate) fn tweak_check<'a>(
        &self,
        scope: Scope,
        label: &'a str,
        which: Tweak,
        on: bool,
    ) -> Element<'a, Message> {
        clickable(
            format!("tweak {label}"),
            tick(on)
                .label(label)
                .on_toggle(move |on| Message::Tweak(scope, which, on))
                .text_size(theme::tap_text(theme::BODY)),
        )
    }

    /// How tall every card on the grid is.
    ///
    /// Pinned, so cards side by side match; it has to clear the fullest card
    /// on the busiest machine, because content past a fixed box is
    /// unreachable. Sized per row (Performance and Looks are shorter than
    /// Input and Advanced), and grown when hints or the Wayland rows appear,
    /// so the page changes height but neighbouring cards always match.
    ///
    /// Every number was measured on screen at the state it names: a card
    /// that clips gives no sign of it. If one outgrows this, raise it here.
    pub(crate) fn card_height(&self, row: CardRow, tweaks: &Tweaks) -> f32 {
        let (base, backend) = match (row, self.hints) {
            // Performance is the taller of the first pair: six switches, the
            // shader-cache note, and the missing-tools line on its floor.
            // Nothing in this row grows with the Wayland backend.
            (CardRow::Top, true) => (300.0, 0.0),
            (CardRow::Top, false) => (250.0, 0.0),
            // Advanced is the taller of the second, and the only card that
            // changes height: the second number is HDR, the monitor field
            // and its own hint, which is why it is larger with hints on.
            (CardRow::Bottom, true) => (486.0, 80.0),
            (CardRow::Bottom, false) => (312.0, 66.0),
        };
        // Those rows only exist once the backend is on, and only where the
        // backend can be offered at all.
        match self.tools.wayland && tweaks.wayland {
            true => base + backend,
            false => base,
        }
    }

    /// One card of the grid: a heading, its own one-line brief, and content.
    ///
    /// Both lengths are `Fill`: the width splits the row evenly, and the
    /// height takes whatever [`Lightforge::card_height`] gave the row.
    ///
    /// **`height(Fill)` only works because the row is a fixed height.** iced's
    /// flex sizes cross-axis-fluid children against their non-fluid siblings
    /// (`layout/flex.rs`, the `cross_compress` path), so two fluid cards in a
    /// Shrink-height row have nothing to measure against, inside a vertical
    /// `scrollable`, where the height limit is unbounded, `cross` stays `0.0`
    /// and both cards draw as nothing at all. That was a real bug, not a
    /// hypothetical. Give the row a height or do not make the cards fluid.
    pub(crate) fn tweak_card<'a>(
        &self,
        title: &'a str,
        brief: &'a str,
        content: iced::widget::Column<'a, Message>,
    ) -> Element<'a, Message> {
        let mut inner = column![text(title).size(theme::HEADING)].spacing(theme::SM);
        inner = inner.extend(self.hint(brief).map(Element::from));
        let content = focus::group(format!("card {title}"), inner.push(content).spacing(theme::SM));
        container(content)
            .padding(theme::MD)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(theme::card)
            .into()
    }

    /// The starting point: one press for the frames-versus-scenery trade.
    ///
    /// Full width and above the grid because it is one decision about
    /// everything under it, and because a segmented control inside a column
    /// would read as a fourth setting rather than the thing that sets them.
    pub(crate) fn preset_row(&self, tweaks: &Tweaks) -> Element<'_, Message> {
        let current = tweaks.preset();

        // The lit one takes the ring too, though pressing it changes
        // nothing: a row the ring skips a segment of reads as broken.
        let segment = |label: &'static str, chosen: bool, message: Message| {
            self.ring(
                focus::Target::Key(format!("preset {label}")),
                Some(message.clone()),
                button(text(label).size(theme::BODY))
                    .padding(theme::tap([theme::SM, theme::MD]))
                    .style(match chosen {
                        true => theme::selected,
                        false => theme::ghost,
                    })
                    .on_press_maybe((!chosen).then_some(message)),
            )
        };

        // Custom first, and lit whenever nothing else is. It is not a fourth
        // preset but the absence of one, which is what makes it stay lit while
        // you tick boxes: setting a switch by hand matches no preset, so this
        // is still the answer. Pressing it clears the managed switches, so the
        // panel it describes really is empty.
        let mut segments = row![segment("Custom", current.is_none(), Message::TweakCustom)]
            .spacing(theme::XS);
        for preset in Preset::ALL {
            segments = segments.push(segment(
                preset.label(),
                current == Some(preset),
                Message::TweakPreset(preset),
            ));
        }

        // The pill repeats what is lit rather than naming a second state:
        // Custom is a real position on the row now, so there is no "neither"
        // left to give its own word.
        let brief = match current {
            Some(preset) => preset.summary().to_owned(),
            None => "Your own mix. Nothing here is set for you.".to_owned(),
        };
        let mut label = column![
            row![
                text("Starting point").size(theme::HEADING),
                pill(
                    match current {
                        Some(preset) => preset.label().to_owned(),
                        None => "Custom".to_owned(),
                    },
                    match current {
                        Some(_) => theme::ACCENT,
                        None => theme::MUTED,
                    },
                ),
            ]
            .spacing(theme::SM)
            .align_y(Alignment::Center),
        ]
        .spacing(theme::XS);
        label = label.extend(self.hint(brief).map(Element::from));

        container(
            row![label.width(Length::Fill), segments]
                .spacing(theme::LG)
                .align_y(Alignment::Center),
        )
        .padding(theme::MD)
        .width(Length::Fill)
        .style(theme::card)
        .into()
    }

    /// Everything that buys frames.
    ///
    /// The host tools are detected, never assumed, so a switch missing here
    /// means the binary is, which is said at the foot of the card rather than
    /// as a hint per switch, because three copies of the same caveat was a
    /// large part of what made this page ragged.
    pub(crate) fn performance_card(&self, tweaks: &Tweaks) -> Element<'_, Message> {
        let mut content = column![].spacing(theme::SM);

        if self.tools.game_performance.is_some() {
            content = content.push(self.tweak_check(Scope::Product,
                "Hold the CPU on its performance profile",
                Tweak::GamePerformance,
                tweaks.wants_game_performance(),
            ));
        }
        if self.tools.gamemode.is_some() {
            content = content.push(self.tweak_check(Scope::Product,
                "Launch through GameMode",
                Tweak::GameMode,
                tweaks.gamemode,
            ));
        }
        content = content.push(self.tweak_check(Scope::Product,
            "Stop profiling addon execution",
            Tweak::SkipAddonProfiling,
            tweaks.skip_addon_profiling,
        ));
        content = content.push(self.tweak_check(Scope::Product,
            "Drop water and weather detail in raids and battlegrounds",
            Tweak::ReduceRaidEffects,
            tweaks.reduce_raid_effects,
        ));
        content = content.push(self.tweak_check(Scope::Product,
            "Skip non-critical world preloading",
            Tweak::FasterZoneLoads,
            tweaks.faster_zone_loads,
        ));
        content = content.push(self.tweak_check(Scope::Product,
            "Turn off cloth and NPC physics",
            Tweak::NoClothPhysics,
            tweaks.no_cloth_physics,
        ));
        content = content.push(self.tweak_check(Scope::Product,
            "Give the shader cache room",
            Tweak::BigShaderCache,
            tweaks.big_shader_cache,
        ));
        content = content.extend(
            self.hint(
                "Mesa's default ceiling is small enough that a long session throws away \
                 shaders it is about to need again, which is felt as stutter that never \
                 settles. Raises it to 10 GB.",
            )
            .map(Element::from),
        );

        // The box is a fixed height now, so this sits on its floor rather
        // than straight under the last switch.
        content = content.push(Space::new().height(Length::Fill));
        let missing: Vec<&str> = [
            self.tools.game_performance.is_none().then_some("game-performance"),
            self.tools.gamemode.is_none().then_some("GameMode"),
        ]
        .into_iter()
        .flatten()
        .collect();
        if !missing.is_empty() {
            content = content.push(muted(
                format!("{} is not installed, so it is not offered.", missing.join(" and ")),
                theme::CAPTION,
            ));
        }

        self.tweak_card("Performance", "Everything here buys frames.", content)
    }

    /// Everything that spends frames, including the optional downloads.
    ///
    /// The glow switch is phrased as "turn off", which reads oddly beside
    /// "Maximum view distance" and was briefly inverted to match it. That was
    /// wrong twice over: the handler kept writing the raw value, so the box
    /// sprang back and looked dead; and once fixed, an empty box meant
    /// `no_screen_effects = true`, which writes `ffxGlow 0`. Custom clears
    /// every switch, so an inverted box would leave Custom visibly empty while
    /// quietly turning the glow off. A box on this panel has to mean "unset"
    /// when empty, and that settles the wording.
    pub(crate) fn looks_card(&self, tweaks: &Tweaks) -> Element<'_, Message> {
        let mut content = column![
            self.tweak_check(Scope::Product,
                "Maximum view distance and ground clutter",
                Tweak::MaxDrawDistance,
                tweaks.max_draw_distance,
            ),
            self.tweak_check(Scope::Product,
                "Turn off the full-screen glow and death filter",
                Tweak::NoScreenEffects,
                tweaks.no_screen_effects,
            ),
        ]
        .spacing(theme::SM);

        // Optional downloads used to be a card of its own, which put a storage
        // decision on an optimization page and left a 70px box next to a 300px
        // one. It is the same trade as the two above, scenery for something
        // else, so it sits with them, and says what it costs.
        let profile = self.config.product(&self.selected.code).map(|p| &p.profile);
        let known = profile.is_some_and(|p| !p.optional_available.is_empty());
        let available =
            offered_only(profile.map(|p| p.optional_available.as_slice()).unwrap_or(&[]));
        let opted = self.opted_optional(&self.selected.code);

        match (known, available.is_empty()) {
            (false, _) => {
                content = content.extend(
                    self.hint(
                        "What else a build offers is only known once its manifest has been \
                         read — use Check for Updates, under the cog beside PLAY.",
                    )
                    .map(Element::from),
                );
            }
            (true, true) => {}
            (true, false) => {
                for name in &available {
                    let on = opted.iter().any(|n| n == name);
                    let tag = name.clone();
                    content = content.push(clickable(
                        format!("optional {name}"),
                        tick(on)
                            .label(optional_label(name))
                            .on_toggle(move |on| Message::OptionalContent(tag.clone(), on))
                            .text_size(theme::tap_text(theme::BODY)),
                    ));
                }
                content = content.extend(
                    self.hint(
                        "Off unless asked for, the same default Battle.net uses. Turning one \
                         on adds it to the next download.",
                    )
                    .map(Element::from),
                );
            }
        }

        content = content.push(Space::new().height(Length::Fill));
        self.tweak_card(
            "Looks",
            "Everything here spends frames. Same trade, stated once.",
            content,
        )
    }

    /// How it feels to play, which is not how fast it runs.
    ///
    /// Four of the five are Wine or multi-monitor fixes rather than
    /// preferences, which is why they sit together: each one is the answer to
    /// a specific thing the game does wrong under Linux, and every name here
    /// was read out of the client's own string table rather than a wiki,
    /// `HardwareCursor` is real and `gxCursor`, which the widely-copied lists
    /// name, is not.
    pub(crate) fn input_card(&self, tweaks: &Tweaks) -> Element<'_, Message> {
        let mut content = column![self.tweak_check(Scope::Product,
            "Raw mouse input",
            Tweak::RawMouse,
            tweaks.raw_mouse,
        )]
        .spacing(theme::SM);
        content = content.extend(
            self.hint(
                "Also the fix for the cursor jumping back to the middle of the window after \
                 a click, which is a Wine problem rather than a game one.",
            )
            .map(Element::from),
        );
        content = content.push(self.tweak_check(Scope::Product,
            "Keep the pointer inside the window",
            Tweak::ClipCursor,
            tweaks.clip_cursor,
        ));
        content = content.extend(
            self.hint(
                "Stops the cursor walking onto another monitor mid-fight. Only worth it on \
                 more than one screen.",
            )
            .map(Element::from),
        );
        content = content.push(self.tweak_check(Scope::Product,
            "Draw a software cursor",
            Tweak::SoftwareCursor,
            tweaks.software_cursor,
        ));
        content = content.extend(
            self.hint(
                "The hardware cursor is the one that goes missing or lags behind the pointer \
                 under Wine. A software one also shows up in screenshots.",
            )
            .map(Element::from),
        );
        content = content.push(self.tweak_check(Scope::Product,
            "Ignore the desktop's mouse acceleration",
            Tweak::NoMouseAcceleration,
            tweaks.no_mouse_acceleration,
        ));
        content = content.push(self.tweak_check(Scope::Product,
            "Show the cursor in relative mode",
            Tweak::ShowCursorInRelativeMode,
            tweaks.show_cursor_in_relative_mode,
        ));
        content = content.extend(
            self.hint(
                "The client's own suggestion for relative-mode bugs, in its words: it \
                 \"may work around issues in some rare cases\".",
            )
            .map(Element::from),
        );
        // Forever only, for now: the line played from a couch or a Deck.
        if self.catalog.line_of(&self.selected.code) == ProductLine::Forever {
            content = content.push(self.tweak_check(Scope::Product,
                "Controller support",
                Tweak::Controller,
                tweaks.controller,
            ));
            content = content.extend(
                self.hint(
                    "Turns on the game's own gamepad mode, which is otherwise only reachable \
                     from the console. Bindings and the on-screen layout are set in-game.",
                )
                .map(Element::from),
            );
        }

        content = content.push(Space::new().height(Length::Fill));
        self.tweak_card("Input and feel", "Nothing here changes how fast it runs.", content)
    }

    /// The fork and the ceiling: choices with no better default.
    pub(crate) fn advanced_card(&self, tweaks: &Tweaks) -> Element<'_, Message> {
        let apis = vec![
            ApiChoice(None),
            ApiChoice(Some(GraphicsApi::D3d11)),
            ApiChoice(Some(GraphicsApi::D3d12)),
        ];
        let mut content = column![row![
            muted("Graphics API".to_owned(), theme::LABEL).width(Length::Fixed(110.0)),
            self.picker(
                "graphics-api".to_owned(),
                apis,
                Some(ApiChoice(tweaks.graphics_api)),
                "",
                Message::TweakApi,
                Length::Fill,
                theme::BODY,
            ),
        ]
        .spacing(theme::MD)
        .align_y(Alignment::Center)]
        .spacing(theme::SM);
        content = content.extend(
            self.hint(
                "A genuine fork rather than a best value: D3D12 runs on VKD3D, D3D11 on \
                 DXVK, and which is faster depends on the card.",
            )
            .map(Element::from),
        );
        content = content.push(
            row![
                muted("Frame cap".to_owned(), theme::LABEL).width(Length::Fixed(110.0)),
                clickable(
                    "frame cap",
                    text_input("uncapped", &self.fps_input)
                        .on_input(Message::TweakFpsCap)
                        .style(theme::field)
                        .size(theme::BODY)
                        .padding(theme::SM)
                        .width(Length::Fixed(110.0)),
                ),
                muted("fps".to_owned(), theme::LABEL),
            ]
            .spacing(theme::MD)
            .align_y(Alignment::Center),
        );
        content = content.extend(
            self.hint(
                "The client's own limit rather than the driver's, so it keeps working if the \
                 Direct3D version above changes. The client's floor is 8.",
            )
            .map(Element::from),
        );
        content = content.push(
            row![
                muted("When unfocused".to_owned(), theme::LABEL).width(Length::Fixed(110.0)),
                clickable(
                    "frame cap unfocused",
                    text_input("30", &self.fps_bk_input)
                        .on_input(Message::TweakFpsCapBackground)
                        .style(theme::field)
                        .size(theme::BODY)
                        .padding(theme::SM)
                        .width(Length::Fixed(110.0)),
                ),
                muted("fps".to_owned(), theme::LABEL),
            ]
            .spacing(theme::MD)
            .align_y(Alignment::Center),
        );
        content = content.extend(
            self.hint(
                "What the game may draw while you are in a browser. The client's own \
                 default is 30; lower gives the rest of the machine more back.",
            )
            .map(Element::from),
        );
        content = content.extend(self.runtime_rows(Scope::Product, tweaks));
        if self.tools.mangohud.is_some() {
            content = content.push(self.tweak_check(Scope::Product,
                "Show the MangoHud overlay",
                Tweak::MangoHud,
                tweaks.mangohud,
            ));
        }

        // An action rather than a setting, so it sits at the foot of the card
        // under the switches, and it belongs with the graphics fork above it,
        // because changing the Direct3D version is the usual reason to want it.
        content = content.push(Space::new().height(Length::Fill));
        content = content
            .push(secondary("Clear shader cache", Message::ClearShaderCache).enabled(true));
        content = content.extend(
            self.hint(
                "Shaders this product compiled under an older Proton build. Clearing them \
                 costs one session of stutter and affects no other product.",
            )
            .map(Element::from),
        );

        self.tweak_card("Advanced", "Choices with no better default.", content)
    }

    /// The Proton-side switches: which graphics backend the game runs on.
    ///
    /// Wayland is only offered in a Wayland session (Proton ignores
    /// `PROTON_ENABLE_WAYLAND` without `WAYLAND_DISPLAY`), and HDR only under
    /// it, since Proton would set `DXVK_HDR` under XWayland too.
    // `'static` rather than borrowing `&self`: `Column::extend` unifies the
    // element lifetime exactly and the cards hold invariant `Action<'static>`.
    pub(crate) fn runtime_rows(&self, scope: Scope, tweaks: &Tweaks) -> Vec<Element<'static, Message>> {
        let mut rows: Vec<Element<'_, Message>> = Vec::new();
        if !self.tools.wayland {
            return rows;
        }

        rows.push(
            self.tweak_check(scope, "Run on Proton's Wayland backend", Tweak::Wayland, tweaks.wayland),
        );
        rows.extend(
            self.hint(
                "Skips XWayland. Newer and less travelled than the X11 path, so it is off \
                 until asked for.",
            )
            .map(Element::from),
        );
        if tweaks.wayland {
            rows.push(self.tweak_check(scope, "Ask for HDR output", Tweak::Hdr, tweaks.hdr));
            rows.push(
                row![
                    muted("Monitor".to_owned(), theme::LABEL).width(Length::Fixed(110.0)),
                    clickable(
                        "monitor",
                        text_input(
                            "whichever Proton picks",
                            match scope {
                                Scope::Product => &self.monitor_input,
                                Scope::BattleNet => &self.bnet_monitor_input,
                            },
                        )
                        .on_input(move |v| Message::TweakWaylandMonitor(scope, v))
                        .style(theme::field)
                        .size(theme::BODY)
                        .padding(theme::SM)
                        .width(Length::Fill),
                    ),
                ]
                .spacing(theme::MD)
                .align_y(Alignment::Center)
                .into(),
            );
            rows.extend(
                self.hint("A connector name, as in DP-1 or HDMI-A-1.").map(Element::from),
            );
        }
        rows
    }

    /// The escape hatch, full width and folded away.
    ///
    /// Out of the grid and collapsed by default because it is the only thing
    /// on the page with no bound on its height: a profile with a dozen
    /// variables in it would make its column twice its neighbour's, which is
    /// the exact problem the grid is here to fix.
    pub(crate) fn env_card(&self, scope: Scope, tweaks: &Tweaks) -> Element<'_, Message> {
        let count = tweaks.env.len();
        let open = match scope {
            Scope::Product => self.env_open,
            Scope::BattleNet => self.bnet_env_open,
        };
        let summary = row![tappable(
            "environment",
            Some(Message::TweakEnvToggle(scope)),
            button(
                text(match open {
                    true => "Hide",
                    false => "Show",
                })
                .size(theme::LABEL)
            )
            .style(theme::ghost)
            .padding(theme::tap([theme::XS, theme::SM])),
        )];
        let mut head = row![text("Environment variables").size(theme::HEADING)]
            .spacing(theme::SM)
            .align_y(Alignment::Center);
        if count > 0 {
            head = head.push(pill(count.to_string(), theme::ACCENT));
        }
        let mut inner = column![row![head.width(Length::Fill), summary]
            .spacing(theme::SM)
            .align_y(Alignment::Center)]
        .spacing(theme::SM);

        if open {
            for (key, value) in &tweaks.env {
                inner = inner.push(
                    container(
                        row![
                            mono(format!("{key}={value}")).width(Length::Fill),
                            tappable(
                                format!("remove variable {key}"),
                                Some(Message::TweakEnvRemove(scope, key.clone())),
                                button(text("Remove").size(theme::LABEL))
                                    .style(theme::ghost)
                                    .padding(theme::tap([theme::XS, theme::SM])),
                            ),
                        ]
                        .spacing(theme::SM)
                        .align_y(Alignment::Center),
                    )
                    .padding([theme::XS, theme::SM])
                    .style(theme::row),
                );
            }
            inner = inner.push(
                row![
                    clickable(
                        "variable",
                        text_input("VARIABLE", &self.env_key_input)
                            .on_input(Message::TweakEnvKey)
                            .on_submit(Message::TweakEnvAdd(scope))
                            .style(theme::field)
                            .size(theme::BODY)
                            .padding(theme::SM)
                            .width(Length::Fixed(220.0)),
                    ),
                    clickable(
                        "value",
                        text_input("value", &self.env_value_input)
                            .on_input(Message::TweakEnvValue)
                            .on_submit(Message::TweakEnvAdd(scope))
                            .style(theme::field)
                            .size(theme::BODY)
                            .padding(theme::SM)
                            .width(Length::Fill),
                    ),
                    secondary("Add", Message::TweakEnvAdd(scope)).enabled(!self.env_key_input.is_empty()),
                ]
                .spacing(theme::SM)
                .align_y(Alignment::Center),
            );
            inner = inner.extend(
                self.hint(
                    "Set for this product's launch only, after everything Lightforge \
                     assembles — so a variable here overrides the launcher's own.",
                )
                .map(Element::from),
            );
        }

        container(inner).padding(theme::MD).width(Length::Fill).style(theme::card).into()
    }
}

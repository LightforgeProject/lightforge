//! The Game Settings popup: launch options, environment, files and backups.

use crate::*;
use iced::widget::{column, text_editor};

impl Lightforge {
    /// The Game Settings popup.
    ///
    /// Rows shaped as in Settings; sections scroll under a fixed title.
    pub(crate) fn game_settings_card<'a>(&'a self, settings: &'a GameSettings) -> Element<'a, Message> {
        // The figure is [`WINDOW_MIN`]'s height less the title, the footer and
        // the padding.
        const SECTIONS_MAX: f32 = 560.0;

        let sections = column![
            section("Performance", self.performance_group()),
            section("Graphics", self.graphics_group(settings)),
            section("Environment", self.environment_group(Scope::Product)),
            section("Game", self.game_group(settings)),
            section("Proton", self.proton_group(settings)),
            section("Addons", self.addons_group()),
            section("Backups", self.backups_group(settings)),
        ]
        .spacing(theme::LG);

        let actions = row![
            Space::new().width(Length::Fill),
            secondary("Close", Message::CloseGameSettings).enabled(true),
        ]
        .align_y(Alignment::Center);

        container(
            column![
                column![
                    text("Game Settings").size(theme::TITLE),
                    muted(settings.label.clone(), theme::LABEL),
                ]
                .spacing(theme::XS),
                container(focus::region(
                    "game settings",
                    scrollable(sections)
                        .direction(scrollable::Direction::Vertical(theme::scrollbar_shape()))
                        .style(theme::scrollbar),
                ))
                .max_height(SECTIONS_MAX),
                actions,
            ]
            .spacing(theme::LG),
        )
        .width(Length::Fixed(680.0))
        .padding(theme::LG)
        .style(theme::dialog)
        .into()
    }

    /// A switch for a tool that may not be installed: dimmed, and said so,
    /// when it is missing, not gone, so nobody wonders where it went.
    pub(crate) fn tool_switch<'a>(
        &self,
        key: &'static str,
        available: bool,
        on: bool,
        scope: Scope,
        which: Tweak,
    ) -> Element<'a, Message> {
        match available {
            true => clickable(
                key,
                iced::widget::toggler(on)
                    .on_toggle(move |on| Message::Tweak(scope, which, on))
                    .size(theme::tap_text(20.0))
                    .style(theme::switch),
            ),
            false => iced::widget::toggler(false)
                .size(theme::tap_text(20.0))
                .style(theme::switch)
                .into(),
        }
    }

    /// CPU profile and overlay, the two things a launch wraps the game in.
    fn performance_group(&self) -> Element<'_, Message> {
        let tweaks = self.tweaks();
        let tools = &self.tools;
        let scope = Scope::Product;

        setting_group([
            setting(
                "CPU performance profile",
                match tools.game_performance {
                    Some(_) => "Holds the CPU on its performance profile while the game runs.",
                    None => "Needs CachyOS' game-performance, which is not installed.",
                },
                self.hint(
                    "game-performance is a small script that sets the CPU's performance \
                     profile for as long as the game runs. On by default wherever it is found.",
                ),
                self.tool_switch(
                    "game performance",
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
                    "mangohud",
                    tools.mangohud.is_some(),
                    tweaks.mangohud && tools.mangohud.is_some(),
                    scope,
                    Tweak::MangoHud,
                ),
            ),
            setting(
                "Shader cache",
                "Compiled shaders kept for this game. Clear it after changing Proton.",
                None,
                secondary("Clear", Message::ClearShaderCache).enabled(true),
            ),
        ])
    }

    /// How the game reaches the screen.
    fn graphics_group<'a>(&'a self, settings: &'a GameSettings) -> Element<'a, Message> {
        let tweaks = self.tweaks();
        let scope = Scope::Product;

        let mut rows = vec![setting(
            "Wayland backend",
            wayland_about(&self.tools),
            self.hint("Newer and less travelled than the X11 path, so it is off until asked for."),
            self.tool_switch(
                "wayland",
                self.tools.wayland,
                tweaks.wayland && self.tools.wayland,
                scope,
                Tweak::Wayland,
            ),
        )];
        if self.tools.hybrid {
            rows.push(setting(
                "Run on the NVIDIA GPU",
                "Renders on the NVIDIA card instead of the integrated one.",
                self.hint("Through the driver's PRIME render offload. Faster, and uses more power."),
                self.tool_switch("nvidia gpu", true, tweaks.nvidia_gpu, scope, Tweak::NvidiaGpu),
            ));
        }
        if self.tools.nvidia.is_some() {
            rows.push(setting(
                "NVIDIA features",
                "Reflex and other NVIDIA-only options, through DXVK-NVAPI.",
                self.hint(
                    "Proton reports an NVIDIA card as an AMD one unless this is on, so games \
                     leave their NVIDIA-only options out.",
                ),
                self.tool_switch("nvapi", true, tweaks.nvapi, scope, Tweak::Nvapi),
            ));
        }

        // Only where the build has textures behind the option.
        if let Some(bytes) = settings.textures_bytes {
            let on = self.opted_optional(&settings.code).iter().any(|n| n == HIGH_RES);
            let working = matches!(
                settings.textures,
                TexturesStep::Planning | TexturesStep::Removing
            );
            let about = match (&settings.textures, on) {
                (TexturesStep::Planning, _) => "Working out what can be deleted…".to_owned(),
                (TexturesStep::Removing, _) => {
                    "Deleting the textures. This rewrites game archives and takes a minute."
                        .to_owned()
                }
                (_, true) => format!("Downloaded and kept up to date, {}.", size_of(bytes)),
                (_, false) => format!(
                    "Sharper textures, {}. Switching on starts the download.",
                    size_of(bytes)
                ),
            };
            let switch: Element<'a, Message> = match working || self.busy {
                true => iced::widget::toggler(on)
                    .size(theme::tap_text(20.0))
                    .style(theme::switch)
                    .into(),
                false => clickable(
                    "high res textures",
                    iced::widget::toggler(on)
                        .on_toggle(|on| Message::OptionalContent(HIGH_RES.to_owned(), on))
                        .size(theme::tap_text(20.0))
                        .style(theme::switch),
                ),
            };
            rows.push(setting(optional_label(HIGH_RES), about, None, switch));

            // The question, in the group it belongs to, once it is asked.
            if let TexturesStep::Ask(plan) = &settings.textures {
                rows.push(setting(
                    "Delete the texture files too?",
                    format!(
                        "Switched off. Deleting them frees {} and leaves everything else alone.",
                        size_of(plan.freed)
                    ),
                    None,
                    row![
                        secondary("Keep them", Message::KeepOptional).enabled(true),
                        action_button("Delete", theme::ERROR, Message::RemoveOptional)
                            .enabled(!self.busy),
                    ]
                    .spacing(theme::SM),
                ));
            }
        }

        setting_group(rows)
    }

    /// The environment-variable editor of one scope, with what is wrong in it.
    pub(crate) fn environment_group(&self, scope: Scope) -> Element<'_, Message> {
        let content = match scope {
            Scope::Product => &self.env_editor,
            Scope::BattleNet => &self.bnet_env_editor,
        };
        let key = match scope {
            Scope::Product => "environment",
            Scope::BattleNet => "bnet environment",
        };
        let (_, bad) = lightforge_core::tweaks::parse_env(&content.text());

        let mut body = column![
            muted("Extra environment variables for the launch, one KEY=value per line.".to_owned(), theme::LABEL),
            clickable(
                key,
                text_editor(content)
                    .placeholder("DXVK_HUD=fps")
                    .on_action(move |action| Message::EnvEdit(scope, action))
                    .height(Length::Fixed(theme::BODY * 1.4 * 5.0 + theme::SM * 2.0))
                    .style(theme::editor)
                    .size(theme::BODY)
                    .padding(theme::SM),
            ),
        ]
        .spacing(theme::SM);

        if !bad.is_empty() {
            let lines: Vec<String> = bad.iter().map(ToString::to_string).collect();
            body = body.push(
                text(format!("Not used: {}", lines.join(", ")))
                    .size(theme::LABEL)
                    .color(theme::ERROR),
            );
        }
        body = body.extend(
            self.hint(
                "Applied last, so a line here wins over the switches above. Lines starting \
                 with # are ignored.",
            )
            .map(Element::from),
        );

        setting_card(body)
    }

    /// Where the game lives, and what can be done to the files there.
    fn game_group<'a>(&'a self, settings: &'a GameSettings) -> Element<'a, Message> {
        let can = self.can_maintain();
        let dir = self.layout.product_dir(&settings.code);
        let shown = match std::env::var_os("HOME").filter(|home| !home.is_empty()) {
            Some(home) => match dir.strip_prefix(&home) {
                Ok(rest) => Path::new("~").join(rest).display().to_string(),
                Err(_) => dir.display().to_string(),
            },
            None => dir.display().to_string(),
        };

        setting_group([
            setting(
                "Location",
                shown,
                None,
                secondary("Show in Explorer", Message::OpenFiles).enabled(true),
            ),
            self.drive_row(settings, can.move_game),
            setting(
                "Game files",
                "Check them against the build and fetch what is damaged.",
                None,
                row![
                    match self.deep_cancel.is_some() {
                        true => secondary("Stop Deep Scan", Message::DeepScan).enabled(true),
                        false => secondary(
                            "Deep Scan",
                            Message::FromGameSettings(Box::new(Message::DeepScan)),
                        )
                        .enabled(can.deep_scan),
                    },
                    secondary(
                        "Scan & Repair",
                        Message::FromGameSettings(Box::new(Message::ScanAndRepair)),
                    )
                    .enabled(can.scan),
                ]
                .spacing(theme::SM),
            ),
            setting(
                "Remove from this computer",
                "Deletes the game's files. Your settings stay in the backups.",
                None,
                action_button(
                    "Uninstall",
                    theme::ERROR,
                    Message::FromGameSettings(Box::new(Message::AskUninstall)),
                )
                .enabled(can.uninstall),
            ),
        ])
    }

    /// Which library the game is on, and moving it to another one.
    fn drive_row<'a>(&'a self, settings: &'a GameSettings, can_move: bool) -> Element<'a, Message> {
        let here = self.layout.library(&settings.code);
        if let Some(moving) = self.moving.as_ref().filter(|m| m.code == settings.code) {
            let progress = moving.progress.lock().map(|p| *p).unwrap_or_default();
            let percent = progress.bytes_done * 100 / progress.bytes_total.max(1);
            let doing = match (moving.releasing, progress.verifying) {
                (true, _) => format!("Removing the old copy from {}…", moving.from.label),
                (false, true) => format!("Checking the copy on {}: {percent}%", moving.to.label),
                (false, false) => format!("Copying to {}: {percent}%", moving.to.label),
            };
            return setting("Drive", doing, None, secondary("Cancel", Message::CancelMove).enabled(!moving.releasing));
        }

        let about = here.map_or_else(String::new, |library| format!("On {}.", library.label));
        let targets: Vec<(String, String)> = self
            .config
            .libraries
            .iter()
            .filter(|library| here.is_none_or(|h| h.id != library.id))
            .map(|library| (library.label.clone(), library.id.clone()))
            .collect();
        if targets.is_empty() {
            return setting("Drive", about, self.hint("Add another drive under Settings › Storage to move the game there."), Space::new());
        }
        let picked = settings
            .move_to
            .as_ref()
            .and_then(|id| targets.iter().find(|(_, t)| t == id))
            .map(|(label, _)| label.clone());
        let labels = targets.iter().map(|(label, _)| label.clone()).collect();
        let picker = self.picker(
            "move to".to_owned(),
            labels,
            picked.clone(),
            "another drive",
            move |label: String| {
                let id = targets.iter().find(|(l, _)| *l == label).map(|(_, id)| id.clone()).unwrap_or_default();
                Message::MoveTargetPicked(id)
            },
            Length::Shrink,
            theme::LABEL,
        );
        setting(
            "Drive",
            about,
            self.hint("Copies the game's files to the other drive, checks every one, then removes the old copy. Nothing is downloaded again."),
            row![picker, secondary("Move", Message::MoveGame).enabled(can_move && picked.is_some())]
                .spacing(theme::SM)
                .align_y(Alignment::Center),
        )
    }

    /// The same pin as Settings › Proton, for whoever is here.
    fn proton_group<'a>(&'a self, settings: &'a GameSettings) -> Element<'a, Message> {
        let code = &settings.code;
        let unpinned = match Kind::of(code) {
            Kind::Managed => Unpinned::Default,
            Kind::HandOff => Unpinned::BattleNet,
        };
        let builds: Vec<ProtonChoice> = std::iter::once(ProtonChoice(None, unpinned))
            .chain(
                self.setup
                    .iter()
                    .flat_map(|s| s.installed.iter())
                    .map(|build| ProtonChoice(Some(build.version.clone()), unpinned)),
            )
            .collect();
        let pin = self.config.product(code).and_then(|p| p.profile.proton_ge_version.clone());
        let for_code = code.clone();

        setting_group([setting(
            "Proton version",
            "The build this game starts with.",
            None,
            self.picker(
                "game settings proton".to_owned(),
                builds,
                Some(ProtonChoice(pin, unpinned)),
                "",
                move |choice| Message::PinProductProton(for_code.clone(), choice),
                Length::Fixed(260.0),
                theme::BODY,
            ),
        )])
    }

    /// The way to the page that manages them.
    fn addons_group(&self) -> Element<'_, Message> {
        setting_group([setting(
            "Installed addons",
            "Update, pin and remove the addons for this game.",
            None,
            secondary(
                "Manage Addons",
                Message::FromGameSettings(Box::new(Message::Show(Screen::Addons))),
            )
            .enabled(true),
        )])
    }

    /// The WTF backups, each restorable or deletable, and the button that
    /// takes one now.
    fn backups_group<'a>(&'a self, settings: &'a GameSettings) -> Element<'a, Message> {
        let idle = !settings.working && !self.busy;

        let mut rows: Vec<Element<'a, Message>> = vec![setting(
            "Your in-game settings",
            "Config.wtf, key bindings and addon data, kept so they can be put back.",
            None,
            secondary("Back up now", Message::BackupNow).enabled(idle),
        )];

        match settings.backups.as_deref() {
            None => rows.push(setting("Reading backups…", "", None, Space::new())),
            Some([]) => rows.push(setting("No backups yet", "", None, Space::new())),
            Some(list) => {
                for backup in list {
                    let asking = settings
                        .confirm
                        .as_ref()
                        .filter(|(path, _)| path == &backup.path)
                        .map(|(_, action)| *action);
                    // The question replaces the details under the date while
                    // it is asked, so the row only ever holds two buttons.
                    let about = match asking {
                        Some(BackupAction::Restore) => {
                            "Replace the current settings with this backup?".to_owned()
                        }
                        Some(BackupAction::Delete) => "Delete this backup?".to_owned(),
                        None => [
                            Some(backup.reason.label().to_owned()),
                            backup.build.clone(),
                            Some(size_of(backup.size)),
                        ]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(" · "),
                    };
                    let controls = match asking {
                        Some(BackupAction::Restore) => row![
                            secondary("Cancel", Message::CancelRestore).enabled(true),
                            action_button("Restore", theme::UPDATE, Message::ConfirmRestore)
                                .enabled(idle),
                        ],
                        Some(BackupAction::Delete) => row![
                            secondary("Cancel", Message::CancelRestore).enabled(true),
                            action_button("Delete", theme::ERROR, Message::ConfirmDeleteBackup)
                                .enabled(idle),
                        ],
                        None => row![
                            secondary("Restore", Message::AskRestore(backup.path.clone()))
                                .enabled(idle),
                            secondary("Delete", Message::AskDeleteBackup(backup.path.clone()))
                                .enabled(idle),
                        ],
                    }
                    .spacing(theme::SM);
                    rows.push(setting(backup.when(), about, None, controls));
                }
            }
        }

        let mut body = column![setting_group(rows)].spacing(theme::SM);
        if let Some(note) = &settings.note {
            body = body.push(match note {
                Ok(line) => muted(line.clone(), theme::LABEL),
                Err(e) => text(e.clone()).size(theme::LABEL).color(theme::ERROR),
            });
        }
        body.into()
    }
}

/// The Wayland backend row's description, in Game Settings and Settings ›
/// Battle.net alike.
pub(crate) fn wayland_about(tools: &HostTools) -> &'static str {
    match tools.wayland {
        false => "Only available in a Wayland session.",
        true if tools.nvidia_lacks_explicit_sync() => {
            "NVIDIA drivers before 555 lack explicit sync, so expect flicker."
        }
        true => "Runs on Proton's Wayland backend instead of XWayland.",
    }
}

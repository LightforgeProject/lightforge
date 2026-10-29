//! General, Addons, Battle.net, Downloads and About.

use crate::*;
use iced::widget::column;

impl Lightforge {
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
    /// the app itself, not about a product, a transfer or a prefix.
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
                super::super::game_settings::wayland_about(tools),
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

    /// Speed caps, ours and Battle.net's, and pausing while playing.
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

    /// The version, where its configuration lives, and the system checks.
    pub(crate) fn about_section(&self) -> Element<'_, Message> {
        let (status, asking) = match (&self.launcher_check, &self.newer_release) {
            (LauncherCheck::Asking, _) => ("Asking GitHub.".to_owned(), true),
            (LauncherCheck::Failed(why), _) => (format!("Could not check: {why}"), false),
            (_, Some(newer)) => (format!("Version {} is available.", newer.latest), false),
            (LauncherCheck::Newest, None) => ("This is the newest version.".to_owned(), false),
            (LauncherCheck::Idle, None) => (String::new(), false),
        };
        let controls = row![text(env!("CARGO_PKG_VERSION")).size(theme::BODY).font(theme::MONO)]
            .push(self.newer_release.as_ref().map(|newer| {
                let label = match self.origin.route(&newer.latest) {
                    lightforge_core::upgrade::Route::Instructions { .. } => "How to update",
                    _ => "Update now",
                };
                action_button(label, theme::ACCENT, Message::UpdateNow).enabled(!self.upgrading())
            }))
            .push(
                secondary(if asking { "Checking" } else { "Check for updates" }, Message::CheckLauncherNow)
                    .enabled(!asking),
            )
            .spacing(theme::MD)
            .align_y(Alignment::Center);
        let mut rows = vec![setting("Version", status, None, controls)];
        if let Some(dirs) = &self.dirs {
            rows.push(setting(
                "Settings file",
                tilde(&dirs.config_file()),
                None,
                secondary("Open folder", Message::OpenConfigFolder).enabled(true),
            ));
        }
        column![setting_group(rows), self.diagnostics_group()].spacing(theme::MD).into()
    }

    /// `lightforge doctor`, one row per check, and a copy for a bug report.
    fn diagnostics_group(&self) -> Element<'_, Message> {
        let running = self.diagnostics == Diagnostics::Running;
        let checks: &[lightforge_core::doctor::Check] = match &self.diagnostics {
            Diagnostics::Done(checks) => checks,
            _ => &[],
        };
        let controls = row![
            secondary(if running { "Checking" } else { "Run checks" }, Message::RunDiagnostics)
                .enabled(!running && self.dirs.is_some()),
            secondary("Copy", Message::CopyDiagnostics).enabled(!checks.is_empty()),
        ]
        .spacing(theme::SM);

        let mut rows = vec![setting(
            "Diagnostics",
            "What this system has, against what Battle.net and the games need.",
            None,
            controls,
        )];
        rows.extend(checks.iter().map(|check| {
            use lightforge_core::doctor::Status;
            let color = match check.status {
                Status::Ok => theme::OK,
                Status::Warn => theme::WARN,
                Status::Fail => theme::ERROR,
            };
            let more = check.hint.as_ref().map(|hint| muted(hint.clone(), theme::CAPTION));
            setting(
                check.name.clone(),
                lightforge_core::debug::scrub(&check.detail),
                more,
                text(check.status.label()).size(theme::LABEL).font(theme::MONO).color(color),
            )
        }));
        setting_group(rows)
    }
}

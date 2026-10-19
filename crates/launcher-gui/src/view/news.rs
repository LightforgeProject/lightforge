//! The News page and Lightforge Updates.

use crate::*;
use iced::widget::column;

impl Lightforge {
    /// Ask for the headlines.
    ///
    /// `force` is the Refresh button. Without it this is a once-per-session
    /// fetch: revalidation is the proxy's job and its answer is cached on
    /// disk, so re-asking on every visit to the tab would buy nothing but
    /// requests.
    pub(crate) fn fetch_news(&mut self, force: bool) -> Task<Message> {
        if self.news_pending || (!force && self.news_asked) {
            return Task::none();
        }
        let Some(dirs) = self.dirs.clone() else {
            return Task::none();
        };
        self.news_pending = true;
        Task::perform(
            tasks::headlines(
                self.http.clone(),
                self.config.news_origin(),
                dirs.news_cache_file(),
            ),
            |headlines| Message::NewsLoaded(headlines.map(Box::new)),
        )
    }

    /// Ask for the pictures of whatever headlines are now on the page.
    pub(crate) fn fetch_news_images(&self) -> Task<Message> {
        let (Some(dirs), Some(headlines)) = (self.dirs.clone(), self.news.as_ref()) else {
            return Task::none();
        };
        let urls: Vec<String> = headlines
            .feed
            .items
            .iter()
            .filter_map(|item| item.image.clone())
            .filter(|url| !url.is_empty())
            .collect();
        if urls.is_empty() {
            return Task::none();
        }

        let have = self.news_images.keys().cloned().collect();
        Task::perform(
            tasks::news_images(self.http.clone(), dirs.news_image_dir(), urls, have),
            Message::NewsImages,
        )
    }

    /// One headline as a card that is entirely a link.
    ///
    /// The whole card presses rather than a "read more" inside it: there is
    /// one thing to do with a headline, and a card with a single small target
    /// in it makes the reader hunt for what the card already is.
    pub(crate) fn news_card<'a>(&self, item: &'a news::Item) -> Element<'a, Message> {
        let mut byline = row![muted(item.source.clone(), theme::LABEL)]
            .spacing(theme::SM)
            .align_y(Alignment::Center);
        if let Some(category) = item.category.as_deref().filter(|c| !c.is_empty()) {
            byline = byline
                .push(muted("·".to_owned(), theme::LABEL))
                .push(muted(category.to_owned(), theme::LABEL));
        }
        byline = byline
            .push(Space::new().width(Length::Fill))
            .push(muted(short_day(&item.published), theme::LABEL));

        let mut words = column![byline, text(item.title.clone()).size(theme::BODY).font(theme::BOLD)]
            .spacing(theme::XS)
            .width(Length::Fill);
        if !item.summary.is_empty() {
            words = words.push(muted(item.summary.clone(), theme::LABEL));
        }

        // The box is drawn whether or not the picture has arrived, so a card
        // does not resize under the reader when it does. An article with no
        // picture at all gets no box: that is a fact about the article, not a
        // load still in flight.
        let card: Element<'_, Message> = match item.image.as_deref() {
            Some(url) if !url.is_empty() => {
                let slot: Element<'_, Message> = match self.news_images.get(url) {
                    Some(handle) => image(handle.clone())
                        .width(Length::Fixed(theme::NEWS_THUMB_WIDTH))
                        .height(Length::Fixed(theme::NEWS_THUMB_HEIGHT))
                        // Cover, not Contain: the box is one shape and these
                        // pictures are not, so the alternative is letterboxing
                        // every card differently.
                        .content_fit(ContentFit::Cover)
                        .into(),
                    None => container(Space::new())
                        .width(Length::Fixed(theme::NEWS_THUMB_WIDTH))
                        .height(Length::Fixed(theme::NEWS_THUMB_HEIGHT))
                        .style(theme::row)
                        .into(),
                };
                row![slot, words].spacing(theme::MD).align_y(Alignment::Center).into()
            }
            _ => words.into(),
        };

        let open = Message::OpenArticle(item.url.clone());
        focus::ring(
            focus::Target::Key(format!("article {}", item.url)),
            focus::Press::Send(open.clone()),
            button(card).padding(theme::MD).width(Length::Fill).style(theme::card_button).on_press(open),
        )
    }

    /// News, headlines from the feeds the proxy merges.
    ///
    /// Every state of the fetch says which one it is. An empty page that
    /// looks broken is worse than one that explains itself, which was true of
    /// the placeholder this replaced and is still the rule here.
    pub(crate) fn news_screen(&self) -> Element<'_, Message> {
        fn centred<'a>(lines: Vec<Element<'a, Message>>) -> Element<'a, Message> {
            container(column(lines).spacing(theme::SM).align_x(Alignment::Center))
                .center(Length::Fill)
                .into()
        }

        let content: Element<'_, Message> = match &self.news {
            Some(headlines) if !headlines.feed.items.is_empty() => {
                let mut list = column![].spacing(theme::SM);

                // Said once, above the headlines, rather than on each card:
                // it is a fact about the page, not about any one article.
                if !headlines.feed.degraded.is_empty() {
                    list = list.push(muted(
                        format!(
                            "{} could not be reached, so some headlines may be missing.",
                            headlines.feed.degraded.join(" and "),
                        ),
                        theme::LABEL,
                    ));
                }
                if headlines.stale {
                    list = list.push(muted(
                        "Showing the last headlines Lightforge fetched — the feed could not be \
                         reached just now."
                            .to_owned(),
                        theme::LABEL,
                    ));
                }

                for item in &headlines.feed.items {
                    list = list.push(self.news_card(item));
                }
                list.into()
            }
            // Asked, answered, and there was nothing in it.
            Some(_) => centred(vec![muted("No headlines right now.".to_owned(), theme::BODY).into()]),
            None if self.news_pending => {
                centred(vec![muted("Fetching headlines…".to_owned(), theme::BODY).into()])
            }
            None if self.news_asked => centred(vec![
                text("No headlines").size(theme::TITLE).into(),
                muted(
                    "The news service could not be reached, and nothing is cached yet.".to_owned(),
                    theme::BODY,
                )
                .into(),
            ]),
            // Before `dirs` exists there is nowhere to cache and nothing has
            // been asked for, the setup screen is what the user is looking at.
            None => centred(vec![muted("Nothing to show yet.".to_owned(), theme::BODY).into()]),
        };

        // Wowhead's own red, and only here: the headlines are theirs, and the
        // button that goes to them is the one place in the app where another
        // brand's colour means something.
        let mut aside = row![action_button("Wowhead", theme::WOWHEAD, Message::OpenLink(WOWHEAD_URL))
            .enabled(true)]
        .spacing(theme::SM)
        .align_y(Alignment::Center);
        if !self.news_pending {
            aside = aside.push(secondary("Refresh", Message::RefreshNews).enabled(true));
        }

        let page = self.page(
            "News",
            Some("Headlines from Wowhead".to_owned()),
            Some(aside.into()),
            content,
        );

        let Some(backdrop) = self.news_bg.clone() else {
            return page;
        };

        // The dashboard's shape: art, a scrim, then the header inside the
        // stack (hoisting it would push the picture below the nav row). The
        // scrim holds dark across the top, where this art is pale sky under
        // the nav row, and settles to [`NEWS_SCRIM`] below, where the
        // translucent cards let the picture through. Offsets run bottom (0.0)
        // to top (1.0).
        stack![
            image(backdrop)
                .content_fit(ContentFit::Cover)
                .width(Length::Fill)
                .height(Length::Fill),
            container(Space::new().height(Length::Fill))
                .width(Length::Fill)
                .style(|_| container::Style {
                    background: Some(iced::Background::Gradient(iced::Gradient::Linear(
                        iced::gradient::Linear::new(0.0)
                            .add_stop(0.0, iced::Color { a: NEWS_SCRIM, ..theme::BG })
                            .add_stop(0.76, iced::Color { a: NEWS_SCRIM, ..theme::BG })
                            .add_stop(0.86, iced::Color { a: NEWS_SCRIM_TOP, ..theme::BG })
                            .add_stop(1.0, iced::Color { a: NEWS_SCRIM_TOP, ..theme::BG }),
                    ))),
                    ..container::Style::default()
                }),
            page,
        ]
        .into()
    }

    /// Lightforge's own release notes. Nothing here yet either.
    pub(crate) fn updates_screen(&self) -> Element<'_, Message> {
        let version = env!("CARGO_PKG_VERSION");
        let releases = lightforge_core::releases::all();

        let mut body = column![].spacing(theme::LG);

        if releases.is_empty() {
            // Only reachable in a build whose notes file was emptied; a test
            // in `core::releases` keeps the shipped one honest.
            body = body.push(muted("No release notes in this build.".to_owned(), theme::BODY));
        }

        for release in &releases {
            let heading = match release.version == version {
                true => format!("{} — this version", release.version),
                false => release.version.clone(),
            };
            body = body.push(
                container(
                    column![text(heading).size(theme::TITLE), release_notes(release)]
                        .spacing(theme::MD),
                )
                .padding(theme::LG)
                .width(Length::Fill)
                .style(theme::row),
            );
        }

        // The notes stay readable here whatever the popup has been told, which
        // is what makes "never show this again" a safe thing to tick.
        body = body.extend(
            self.hint(
                "These are the notes the launcher shows after an update. Settings › General \
                 decides whether that popup appears.",
            )
            .map(Element::from),
        );

        // Through `page` like every other page, which is what gives it the
        // same margins and title; built on its own it sat flush against the
        // window's edges.
        self.page(
            "Lightforge Updates",
            Some(format!("You are running {version}.")),
            None,
            body.into(),
        )
    }
}

//! Where an arrow goes: pure geometry over the places last drawn.

use super::*;

/// Where `step` goes from `current`, among `places`.
///
/// Up/Down stay inside the ring's group first (down a card is the next switch
/// in that card), then go straight above or below. Outside any group (header,
/// game band) they take the next row, then the nearest control sideways;
/// boxes that overlap sideways count as no distance apart. Left/Right stay on
/// the line, reaching off it only outside a group. Nothing in that direction
/// leaves the ring where it is.
pub fn step(places: &[Place], current: &Target, step: Step) -> Option<Target> {
    let here = places.iter().find(|p| &p.target == current)?;
    let from = here.bounds;

    // How far apart two spans are, zero when they overlap.
    let apart = |a0: f32, a1: f32, b0: f32, b1: f32| (b0 - a1).max(a0 - b1).max(0.0);
    // Two pixels of slack: neighbours drawn edge to edge, like PLAY and its
    // cog, share a border rather than leaving a gap.
    const SLACK: f32 = 2.0;
    // How far down a row may straggle and still be one row: a checkbox and
    // a button beside it do not share a top edge.
    const ROW: f32 = 16.0;

    // For each control in that direction: how far ahead, how far aside, and
    // how far apart the centres are, to break ties.
    struct Candidate<'t> {
        place: &'t Place,
        ahead: f32,
        aside: f32,
        centres: f32,
    }
    let candidates: Vec<Candidate> = places
        .iter()
        .filter(|p| &p.target != current)
        .filter(|p| here.pane.is_none() || p.pane == here.pane)
        .filter_map(|p| {
            let to = p.bounds;
            let (ahead, aside, centres) = match step {
                Step::Down => (
                    to.y - (from.y + from.height),
                    apart(from.x, from.x + from.width, to.x, to.x + to.width),
                    (to.center_x() - from.center_x()).abs(),
                ),
                Step::Up => (
                    from.y - (to.y + to.height),
                    apart(from.x, from.x + from.width, to.x, to.x + to.width),
                    (to.center_x() - from.center_x()).abs(),
                ),
                Step::Right => (
                    to.x - (from.x + from.width),
                    apart(from.y, from.y + from.height, to.y, to.y + to.height),
                    (to.center_y() - from.center_y()).abs(),
                ),
                Step::Left => (
                    from.x - (to.x + to.width),
                    apart(from.y, from.y + from.height, to.y, to.y + to.height),
                    (to.center_y() - from.center_y()).abs(),
                ),
            };
            (ahead >= -SLACK).then_some(Candidate { place: p, ahead: ahead.max(0.0), aside, centres })
        })
        .collect();

    let pick = |pool: Vec<&Candidate>, key: &dyn Fn(&Candidate) -> f32| {
        pool.into_iter().min_by(|a, b| key(a).total_cmp(&key(b))).map(|c| c.place.target.clone())
    };
    // The next row in that direction, then the nearest sideways in it.
    let next_row = |pool: Vec<&Candidate>| {
        let nearest = pool.iter().map(|c| c.ahead).min_by(f32::total_cmp)?;
        let row = pool.into_iter().filter(|c| c.ahead <= nearest + ROW).collect();
        pick(row, &|c| c.aside + 0.01 * c.centres)
    };

    match step {
        Step::Up | Step::Down => {
            let Some(group) = &here.group else {
                return next_row(candidates.iter().collect());
            };
            let inside: Vec<_> = candidates.iter().filter(|c| c.place.group.as_ref() == Some(group)).collect();
            if !inside.is_empty() {
                return next_row(inside);
            }
            let straight: Vec<_> = candidates.iter().filter(|c| c.aside == 0.0).collect();
            match straight.is_empty() {
                false => next_row(straight),
                true => next_row(candidates.iter().collect()),
            }
        }
        Step::Left | Step::Right => {
            let line: Vec<_> = candidates.iter().filter(|c| c.aside == 0.0).collect();
            if !line.is_empty() {
                return pick(line, &|c| c.ahead + 0.01 * c.centres);
            }
            // Inside a group, a page or a card, sideways never leaves the
            // line: left from the first control in an addon row is nothing,
            // not the filter above it or the Shop tab up in the header,
            // which were the nearest things to the left.
            if here.group.is_some() {
                return None;
            }
            pick(candidates.iter().collect(), &|c| c.ahead + 2.0 * c.aside + 0.01 * c.centres)
        }
    }
}

/// The control whose centre is nearest `at`'s.
pub fn nearest(places: &[Place], at: Rectangle) -> Option<Target> {
    let far = |p: &Place| p.bounds.center().distance(at.center());
    places.iter().min_by(|a, b| far(a).total_cmp(&far(b))).map(|p| p.target.clone())
}

/// The control nearest the top left, where reading starts.
pub fn first(places: &[Place]) -> Option<Target> {
    places
        .iter()
        .min_by(|a, b| a.bounds.y.total_cmp(&b.bounds.y).then(a.bounds.x.total_cmp(&b.bounds.x)))
        .map(|p| p.target.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(x: f32, y: f32, width: f32, height: f32) -> Rectangle {
        Rectangle { x, y, width, height }
    }

    /// The dashboard's shape at Deck size, from a screenshot: the header,
    /// the band, the panel. Nothing on it is grouped.
    fn page() -> Vec<Place> {
        let places = vec![
            (Target::AppMenu, at(40.0, 10.0, 240.0, 30.0)),
            (Target::Tab(Screen::Dashboard), at(390.0, 10.0, 40.0, 30.0)),
            (Target::Shop, at(455.0, 10.0, 50.0, 30.0)),
            (Target::Tile(ProductLine::Retail), at(110.0, 58.0, 56.0, 48.0)),
            (Target::Tile(ProductLine::Classic), at(236.0, 58.0, 56.0, 48.0)),
            (Target::Tile(ProductLine::ClassicEra), at(300.0, 58.0, 56.0, 48.0)),
            (Target::Tile(ProductLine::Forever), at(385.0, 58.0, 56.0, 48.0)),
            (Target::BattleNet, at(1135.0, 66.0, 90.0, 32.0)),
            (Target::Key("picker version".to_owned()), at(107.0, 668.0, 56.0, 22.0)),
            (Target::Addons, at(62.0, 722.0, 118.0, 24.0)),
            (Target::Primary, at(983.0, 660.0, 185.0, 56.0)),
            (Target::Gear, at(1168.0, 660.0, 50.0, 56.0)),
            (Target::LaunchViaBattleNet, at(983.0, 722.0, 235.0, 30.0)),
        ];
        places.into_iter().map(|(target, bounds)| Place { target, bounds, group: None, pane: None }).collect()
    }

    /// Two cards side by side, the right one's rows sitting a little lower,
    /// as a page of settings cards draws them.
    #[test]
    fn down_a_card_stays_in_the_card() {
        let place = |name: &str, x: f32, y: f32, group: &str| Place {
            target: Target::Key(name.to_owned()),
            bounds: at(x, y, 200.0, 20.0),
            group: Some(group.to_owned()),
            pane: None,
        };
        let cards = vec![
            Place { target: Target::AppMenu, bounds: at(40.0, 10.0, 240.0, 30.0), group: None, pane: None },
            place("custom", 880.0, 60.0, "presets"),
            place("cpu", 50.0, 120.0, "performance"),
            place("water", 50.0, 160.0, "performance"),
            place("clutter", 660.0, 120.0, "looks"),
            place("glow", 660.0, 140.0, "looks"),
            place("api", 660.0, 300.0, "advanced"),
        ];
        let key = |name: &str| Some(Target::Key(name.to_owned()));
        // Out of the header: the next row, the presets, however far right.
        assert_eq!(step(&cards, &Target::AppMenu, Step::Down), key("custom"));
        // Down the Looks card, then out of it straight down, not across.
        assert_eq!(step(&cards, &Target::Key("clutter".to_owned()), Step::Down), key("glow"));
        assert_eq!(step(&cards, &Target::Key("glow".to_owned()), Step::Down), key("api"));
    }

    /// An addon row inside the page: left from its first control is nothing,
    /// where it used to be the Shop tab or the filter field, whichever was
    /// the nearest thing to the left.
    #[test]
    fn left_out_of_a_page_row_goes_nowhere() {
        let page = |name: &str, x: f32, y: f32| Place {
            target: Target::Key(name.to_owned()),
            bounds: at(x, y, 80.0, 30.0),
            group: Some("page".to_owned()),
            pane: None,
        };
        let places = vec![
            Place { target: Target::Shop, bounds: at(350.0, 10.0, 120.0, 30.0), group: None, pane: None },
            page("filter", 20.0, 120.0),
            page("details", 1000.0, 300.0),
            page("install", 1100.0, 300.0),
            page("next details", 1000.0, 360.0),
        ];
        let details = Target::Key("details".to_owned());
        assert_eq!(step(&places, &details, Step::Left), None);
        assert_eq!(step(&places, &details, Step::Right), Some(Target::Key("install".to_owned())));
    }

    /// Settings: the list of pages down the left, the page beside it. The
    /// arrows walk the page and never leave it, not down from the last
    /// switch onto the list's next entry, not left onto the list.
    #[test]
    fn the_arrows_never_leave_a_pane() {
        let tab = |name: &str, y: f32| Place {
            target: Target::Key(name.to_owned()),
            bounds: at(20.0, y, 200.0, 36.0),
            group: Some("settings nav".to_owned()),
            pane: None,
        };
        let switch = |name: &str, y: f32| Place {
            target: Target::Key(name.to_owned()),
            bounds: at(300.0, y, 300.0, 20.0),
            group: Some("settings".to_owned()),
            pane: Some("settings"),
        };
        let places = vec![
            tab("general", 100.0),
            tab("downloads", 300.0),
            tab("addons", 340.0),
            switch("minimize", 120.0),
            switch("new version", 240.0),
        ];
        let key = |name: &str| Some(Target::Key(name.to_owned()));
        let last = Target::Key("new version".to_owned());
        assert_eq!(step(&places, &last, Step::Down), None);
        assert_eq!(step(&places, &last, Step::Left), None);
        assert_eq!(step(&places, &last, Step::Up), key("minimize"));
        // The list itself is no pane: right from it still finds the page.
        assert_eq!(step(&places, &Target::Key("general".to_owned()), Step::Right), key("minimize"));
    }

    #[test]
    fn up_from_the_first_game_is_the_logo_above_it() {
        let page = page();
        assert_eq!(step(&page, &Target::Tile(ProductLine::Retail), Step::Up), Some(Target::AppMenu));
        assert_eq!(step(&page, &Target::Tile(ProductLine::Classic), Step::Up), Some(Target::AppMenu));
        // Under a tab rather than the wordmark, the tab straight above wins.
        assert_eq!(
            step(&page, &Target::Tile(ProductLine::Forever), Step::Up),
            Some(Target::Tab(Screen::Dashboard))
        );
    }

    #[test]
    fn down_goes_to_what_is_below_not_to_a_fixed_place() {
        let page = page();
        // The next row down is the version picker's, not the links under it.
        assert_eq!(
            step(&page, &Target::Tile(ProductLine::Retail), Step::Down),
            Some(Target::Key("picker version".to_owned()))
        );
        assert_eq!(step(&page, &Target::Key("picker version".to_owned()), Step::Down), Some(Target::Addons));
        // Down from the header is the next row, however far to the side.
        assert_eq!(step(&page, &Target::AppMenu, Step::Down), Some(Target::Tile(ProductLine::Retail)));
        assert_eq!(step(&page, &Target::Shop, Step::Down), Some(Target::Tile(ProductLine::Forever)));
        assert_eq!(step(&page, &Target::BattleNet, Step::Down), Some(Target::Gear));
        assert_eq!(step(&page, &Target::Primary, Step::Down), Some(Target::LaunchViaBattleNet));
        assert_eq!(step(&page, &Target::Primary, Step::Right), Some(Target::Gear));
    }

    #[test]
    fn nothing_in_that_direction_stays_put() {
        let page = page();
        assert_eq!(step(&page, &Target::AppMenu, Step::Up), None);
        assert_eq!(step(&page, &Target::AppMenu, Step::Left), None);
        assert_eq!(first(&page), Some(Target::AppMenu));
    }
}

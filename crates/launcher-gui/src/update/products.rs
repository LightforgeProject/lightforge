//! The game band and the products in it: tiles, branches, themes, the installed registry.

use crate::*;

impl Lightforge {
    pub(crate) fn update_products(&mut self, message: Message) -> Result<Task<Message>, Message> {
        Ok(match message {
            Message::LineSelected(line) => {
                // Picking from the overflow menu closes it: the tile is in the
                // band now, and the menu is answering a question already asked.
                self.close_panels();
                // The band is on News too, where choosing a game is a request
                // to go and look at it, leaving you on a page that says
                // nothing about the game you just picked would read as a dead
                // click.
                self.go_to(Screen::Dashboard);
                self.select_line(&line)
            }
            Message::BranchSelected(pick) => {
                // An unconfigured branch is offered, not hidden, picking it
                // is how it gets added, which beats hand-editing config.toml
                // to try a PTR for an afternoon.
                if !pick.configured {
                    let label = pick.label.clone();
                    self.config
                        .products
                        .push(Product::new(pick.code.clone(), label.clone()));
                    self.save_config();
                    self.choices.push(Choice { code: pick.code.clone(), label });
                }
                let Some(choice) = self.choices.iter().find(|c| c.code == pick.code).cloned() else {
                    return Ok(Task::none());
                };
                self.select(choice)
            }
            Message::Refreshed(code, refreshed) => {
                if code != self.selected.code {
                    return Ok(Task::none());
                }
                // A drive can go away between two refreshes, and this is the
                // moment the panel is being rebuilt anyway.
                self.check_drives();
                self.local = refreshed.local.clone();
                self.remote = refreshed.remote.clone();
                self.status = match &refreshed.offline {
                    // Rule 2 of the resolver: offline never blocks a launch, so
                    // it is a note, not an error.
                    Some(_) => "version service unreachable — offline".to_owned(),
                    None => refreshed
                        .remote
                        .as_ref()
                        .map(|r| format!("latest {}", r.versions_name))
                        .unwrap_or_default(),
                };
                Task::none()
            }
            Message::CachedTheme(code, themed) => {
                trace::mark(format_args!(
                    "cache-first theme for {}: {}, {} icon(s)",
                    code.as_str(),
                    match &themed.branding {
                        Some(_) => "hit",
                        None => "MISS — nothing usable on disk",
                    },
                    themed.icons.len(),
                ));
                // Strictly a gap-filler: the revalidated answer wins whenever
                // it happens to arrive first, so none of this overwrites it.
                if self.manifest.is_none() {
                    self.manifest = themed.manifest;
                    self.refresh_catalog();
                }
                if self.icons.is_empty() {
                    self.icons = themed.icons;
                }
                let selected = code == self.selected.code;
                if selected && self.branding.is_none() {
                    self.branding = themed.branding;
                }
                match selected {
                    true => self.splash_decide(themed.warm),
                    false => Task::none(),
                }
            }
            Message::Themed(code, themed) => {
                trace::mark(format_args!(
                    "revalidated theme for {}: manifest {}, branding {}",
                    code.as_str(),
                    if themed.manifest.is_some() { "ok" } else { "failed" },
                    if themed.branding.is_some() { "ok" } else { "failed" },
                ));
                // The manifest describes every product, so it is kept even
                // when a switch has already moved on from the one it themed.
                if themed.manifest.is_some() {
                    self.manifest = themed.manifest;
                    self.refresh_catalog();
                }
                // Same reasoning as the branding swap below. A revalidation
                // that answered 304 produces the same files decoded again;
                // assigning them hands the renderer brand-new texture ids for
                // pixels already on screen, and the whole band blinks after
                // every switch.
                if !themed.icons.is_empty() && !tasks::same_marks(&self.icons, &themed.icons) {
                    self.icons = themed.icons;
                }
                if code == self.selected.code {
                    // Only when it is actually different art. A revalidation
                    // that answered 304 resolves to the same files, already
                    // decoded and on screen, but assigning it hands iced new
                    // image handles, which it has to upload again, and the
                    // page blinks a second or two after a switch for no
                    // reason. [`Branding`] carries each asset's path and
                    // whether its bytes moved this run, so comparing it
                    // answers exactly that question.
                    let same = self.branding.as_ref().map(|art| &art.branding)
                        == themed.branding.as_ref().map(|art| &art.branding);
                    trace::mark(match same {
                        true => "branding unchanged, keeping the decoded art",
                        false => "branding changed, swapping the art",
                    });
                    if !same {
                        self.branding = themed.branding;
                    }
                }
                Task::none()
            }
            Message::ToggleManager => {
                self.manager_open = !self.manager_open;
                if self.manager_open {
                    self.browse_open = false;
                    self.strip_open = false;
                }
                Task::none()
            }
            Message::ToggleStrip => {
                self.strip_open = !self.strip_open;
                if self.strip_open {
                    self.browse_open = false;
                    self.manager_open = false;
                }
                Task::none()
            }
            Message::OpenProduct(code) => {
                self.close_panels();
                self.go_to(Screen::Dashboard);
                let choice = self.choices.iter().find(|c| c.code == code).cloned().unwrap_or_else(|| {
                    let label = self
                        .catalog
                        .get(&code)
                        .map(|e| e.label.clone())
                        .unwrap_or_else(|| code.as_str().to_owned());
                    Choice { code, label }
                });
                self.select(choice)
            }
            Message::TileMenu(line) => {
                let open = self.tile_menu.as_ref() != Some(&line);
                self.close_panels();
                self.tile_menu = open.then_some(line);
                Task::none()
            }
            Message::TileEntered(line) => {
                self.tile_hover = Some(line);
                Task::none()
            }
            Message::TileLeft(line) => {
                // Only if it is still the one recorded: entering the next tile
                // can arrive before leaving the last.
                if self.tile_hover.as_ref() == Some(&line) {
                    self.tile_hover = None;
                }
                Task::none()
            }
            Message::TileGrabbed(line) => {
                self.tile_drag = Some(TileDrag { line, origin: None, last: 0.0, moving: false });
                Task::none()
            }
            Message::TileDragged(x) => {
                let Some(drag) = &mut self.tile_drag else {
                    return Ok(Task::none());
                };
                let Some(origin) = drag.origin else {
                    drag.origin = Some(x);
                    drag.last = x;
                    return Ok(Task::none());
                };
                drag.moving |= (x - origin).abs() > DRAG_SLOP;
                let heading = x - drag.last;
                drag.last = x;
                if !drag.moving {
                    return Ok(Task::none());
                }

                // Moved over another tile: take its place, the way a browser
                // tab does. Only in the direction of travel, or two tiles of
                // different widths trade places back and forth under a
                // cursor that has come to rest on the seam.
                let held = drag.line.clone();
                let Some(over) = self.tile_hover.clone().filter(|line| line != &held) else {
                    return Ok(Task::none());
                };
                let lines = self.lines();
                let (Some(from), Some(to)) = (
                    lines.iter().position(|l| l == &held),
                    lines.iter().position(|l| l == &over),
                ) else {
                    return Ok(Task::none());
                };
                if (to > from && heading > 0.0) || (to < from && heading < 0.0) {
                    self.move_line(from, to);
                }
                Task::none()
            }
            Message::TileDropped => {
                let Some(drag) = self.tile_drag.take() else {
                    return Ok(Task::none());
                };
                match drag.moving {
                    // Saved once, at the end: the order changes on every
                    // tile crossed, and the file needs only the last one.
                    true => {
                        self.save_config();
                        Task::none()
                    }
                    // A click is a press and a release on the same tile.
                    false if self.tile_hover.as_ref() == Some(&drag.line)
                        && drag.line != self.catalog.line_of(&self.selected.code) =>
                    {
                        self.update(Message::LineSelected(drag.line))
                    }
                    false => Task::none(),
                }
            }
            Message::ToggleBrowse => {
                self.browse_open = !self.browse_open;
                self.strip_open = false;
                if !self.browse_open {
                    return Ok(Task::none());
                }
                self.manager_open = false;
                // Read on open, so the list says what is already on the
                // machine without anything running in the background.
                Task::perform(
                    tasks::installed_versions(self.layout.clone()),
                    Message::InstalledVersions,
                )
            }
            Message::InstalledVersions(found) => {
                self.installed_versions = found;
                Task::none()
            }
            Message::RegistryPoll => {
                let Some(watch) = &mut self.registry_watch else { return Ok(Task::none()) };
                watch.polls += 1;
                let check_client = watch.polls % REGISTRY_CLIENT_EVERY == 0;
                Task::perform(tasks::registry_probe(self.layout.clone(), check_client), |(stamp, running)| {
                    Message::RegistryPolled(stamp, running)
                })
            }
            Message::RegistryPolled(stamp, running) => {
                let Some(watch) = &mut self.registry_watch else { return Ok(Task::none()) };
                let changed = watch.stamp != stamp;
                watch.stamp = stamp;
                match running {
                    Some(true) => watch.seen_client = true,
                    Some(false) if watch.seen_client || watch.since.elapsed() > REGISTRY_GRACE => {
                        self.registry_watch = None;
                    }
                    _ => {}
                }
                match changed {
                    // The game families come from the client's cached
                    // catalogue, which the same session fills.
                    true => Task::batch([
                        Task::perform(
                            tasks::installed_versions(self.layout.clone()),
                            Message::InstalledVersions,
                        ),
                        Task::perform(
                            tasks::game_families(
                                self.layout.clone(),
                                self.config.prefix_user.clone(),
                                self.config.region,
                            ),
                            |families| Message::FamiliesRead(Box::new(families)),
                        ),
                    ]),
                    false => Task::none(),
                }
            }
            Message::KnownCodes(codes) => {
                self.known_codes = codes;
                self.refresh_catalog();
                Task::none()
            }
            Message::RemoveProduct(code) => {
                // A tile is a *line*, and its PTR and beta are branches behind
                // its version picker rather than games of their own, so
                // Remove takes the line with it. Dropping the head alone left
                // the tile on screen wearing whichever branch survived, which
                // is what someone removing the game has just said they do not
                // want.
                let line = self.catalog.line_of(&code);
                let going: Vec<ProductCode> = self
                    .choices
                    .iter()
                    .filter(|c| self.catalog.line_of(&c.code) == line)
                    .map(|c| c.code.clone())
                    .collect();

                // The header cannot be empty: with nothing tracked there is no
                // product to show and no way back to this panel.
                if going.len() >= self.choices.len() {
                    self.error =
                        Some("that is the only game left — add another first".into());
                    return Ok(Task::none());
                }

                for code in &going {
                    // A transfer for a product nobody is tracking has nowhere
                    // to report to, so it stops here rather than running on
                    // unseen.
                    self.pause(code);
                    self.queue.retain(|d| &d.code != code);
                    self.downloads.finished(code);
                }

                self.config.products.retain(|p| !going.contains(&p.code));
                self.save_config();
                self.choices.retain(|c| !going.contains(&c.code));
                self.tile_menu = None;
                self.last_branch.remove(&line);

                match going.contains(&self.selected.code) {
                    // Standing on one of the ones just removed: move to
                    // whatever the header now starts with.
                    true => {
                        let next = self.choices[0].clone();
                        self.select(next)
                    }
                    false => Task::none(),
                }
            }
            Message::AddProduct(code) => {
                if self.choices.iter().any(|c| c.code == code) {
                    return Ok(Task::none());
                }
                let label = self
                    .catalog
                    .get(&code)
                    .map(|e| e.label.clone())
                    .unwrap_or_else(|| code.as_str().to_owned());

                self.config.products.push(Product::new(code.clone(), label.clone()));
                self.save_config();
                self.choices.push(Choice { code, label });
                Task::none()
            }
            Message::FamiliesRead(families) => {
                self.families = *families;
                Task::none()
            }

            other => return Err(other),
        })
    }
}

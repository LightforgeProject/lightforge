//! Taking found installs and games into the config.

use super::*;

/// Untick and explain what Lightforge already has: a WoW install whose folder
/// is a library with every product assigned, a Battle.net game already linked.
pub(super) fn mark_known(app: &mut Lightforge) {
    let config = &app.config;
    let default = config.libraries.iter().find(|l| l.is_default);
    let known = |row: &Row| match &row.kind {
        RowKind::Wow(install) => {
            let library = config.libraries.iter().find(|l| Some(l.games()) == install.path.parent());
            library.is_some_and(|library| {
                install.products.iter().all(|code| {
                    config.product(code).is_some_and(|p| p.profile.library.as_deref() == Some(library.id.as_str()))
                })
            })
        }
        RowKind::Game(game) => default.is_some_and(|library| {
            std::fs::read_link(library.games().join(&game.label)).is_ok_and(|to| to == game.path)
        }),
    };
    let Some(wizard) = app.wizard.as_mut() else {
        return;
    };
    for row in wizard.rows.iter_mut().filter(|row| row.blocked.is_none()) {
        if known(row) {
            row.take = false;
            row.blocked = Some("already in Lightforge".to_owned());
        }
    }
}

/// Adopt every ticked WoW install, and remember the ticked hand-offs for later.
///
/// In place: the parent becomes a library, nothing is copied. Hand-offs wait
/// for the prefix and registry the next step creates.
pub(super) fn adopt_taken(app: &mut Lightforge) {
    let Some(rows) = app.wizard.as_ref().map(|w| w.rows.clone()) else {
        return;
    };
    let mut notes = Vec::new();
    let mut pending = Vec::new();

    for row in rows.iter().filter(|row| row.take) {
        match &row.kind {
            RowKind::Wow(install) => match adopt_one(app, install) {
                Ok(mut found) => notes.append(&mut found),
                Err(e) => notes.push(Note::failed(format!(
                    "{} could not be added: {e}",
                    install.path.display()
                ))),
            },
            RowKind::Game(game) => {
                // Into the band now; the link waits for the prefix (`pending`).
                track_hand_off(app, game);
                pending.push((game.clone(), row.prefix.clone()));
            }
        }
    }

    if let Some(wizard) = app.wizard.as_mut() {
        wizard.adopted = notes;
        wizard.pending = pending;
        // Ticked rows have been acted on; leaving them ticked would adopt them
        // again if the user stepped back and forward.
        for row in &mut wizard.rows {
            if row.take {
                row.take = false;
                row.blocked = Some("already added".to_owned());
            }
        }
    }
}

/// Add one found install to the config, in place.
///
/// Returns every line worth showing: adopted, and why Battle.net cannot see it.
fn adopt_one(app: &mut Lightforge, install: &WowInstall) -> Result<Vec<Note>, String> {
    let mut notes = Vec::new();

    let (id, label) = match discover::plan(install, &app.config.libraries) {
        Ok(plan) => {
            let adopted = discover::adopt(&plan, &app.layout).map_err(|e| chain(&e))?;
            let id = adopted.library.id.clone();
            let label = adopted.library.label.clone();
            if adopted.repaired {
                notes.push(Note::warning(format!(
                    "{} would not accept a file; its owner-write permission was added",
                    install.path.display()
                )));
            }
            if let Some(reason) = adopted.battle_net {
                notes.push(Note::warning(format!(
                    "Battle.net was not told about {}, so you may have to use its own \
                     Locate button for this one: {reason}",
                    install.path.display()
                )));
            }
            app.config.libraries.push(adopted.library);
            (id, label)
        }
        // Already in a known library; its products may still need assigning.
        Err(discover::AdoptError::AlreadyOurs) => {
            let parent = install.path.parent().unwrap_or(&install.path);
            let library = app
                .config
                .libraries
                .iter()
                .find(|l| l.games() == parent)
                .ok_or_else(|| "that folder is managed but its drive is gone".to_owned())?;
            (library.id.clone(), library.label.clone())
        }
        Err(e) => return Err(chain(&e)),
    };

    for code in &install.products {
        if !app.config.products.iter().any(|p| &p.code == code) {
            let name = app
                .catalog
                .get(code)
                .map(|e| e.label.clone())
                .unwrap_or_else(|| code.as_str().to_owned());
            app.config.products.push(lightforge_core::Product::new(code.clone(), name));
        }
        if let Some(product) = app.config.product_mut(code) {
            product.profile.library = Some(id.clone());
        }
    }
    app.save_config();
    app.rebuild_layout();
    app.sync_choices();
    // Only with a prefix; otherwise building it links the library.
    if lightforge_core::prefix::PrefixStatus::inspect(&app.layout).initialized {
        if let Err(e) = lightforge_core::prefix::link_libraries(&app.layout) {
            notes.push(Note::warning(format!("the prefix could not be relinked: {}", chain(&e))));
        }
    }
    notes.insert(
        0,
        Note::done(format!(
            "{} — kept where it is, on {label}",
            install
                .products
                .iter()
                .map(|code| code.short_name().to_owned())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    );
    Ok(notes)
}

/// Put a found Battle.net game in the band, if it is not there already.
///
/// On a first run `Catalog::reconcile` may not have run (`hs_beta` vs
/// `hsb`), so two codes are one game when they share a game family.
fn track_hand_off(app: &mut Lightforge, game: &HandOff) {
    let (code, label) = hand_off_entry(&app.catalog, game);
    if app.config.products.iter().any(|p| p.code == code) {
        return;
    }
    app.config.products.push(lightforge_core::Product::new(code, label));
    app.save_config();
    app.sync_choices();
}

/// Which catalog entry a found game is, and what to call it.
///
/// Pure, so the matching can be tested.
pub(super) fn hand_off_entry(
    catalog: &lightforge_core::product::Catalog,
    game: &HandOff,
) -> (ProductCode, String) {
    let found: ProductCode = game.code.clone().into();
    if let Some(entry) = catalog.get(&found) {
        return (found, entry.label.clone());
    }
    let by_family = lightforge_core::battlenet::game_uid(&found).and_then(|family| {
        catalog
            .hand_offs()
            .find(|e| {
                lightforge_core::battlenet::game_uid(&e.code).as_deref() == Some(family.as_str())
            })
            .map(|e| (e.code.clone(), e.label.clone()))
    });
    // A game the catalog has never heard of still goes in, under the name of
    // the folder it was found in, which is what the user recognises.
    by_family.unwrap_or_else(|| (found, game.label.clone()))
}

/// Point our Battle.net at the games another one installed.
pub(super) fn link_hand_offs(app: &mut Lightforge, pending: &[(HandOff, PathBuf)]) {
    let Some(library) = app.config.libraries.iter().find(|l| l.is_default).cloned() else {
        return;
    };
    if app.config.battle_net_declined {
        return;
    }
    let mut linked = Vec::new();
    let mut missed = Vec::new();
    for (game, prefix) in pending {
        match discover::link_hand_off(game, prefix, &app.layout, &library) {
            Ok(true) => linked.push(game.label.clone()),
            // Linked, but Battle.net was not told, it will offer to find the
            // game itself, which works and is not worth an error.
            Ok(false) => missed.push(game.label.clone()),
            Err(e) => missed.push(format!("{} ({e})", game.label)),
        }
    }
    app.status = match (linked.is_empty(), missed.is_empty()) {
        (true, true) => return,
        (false, true) => format!("Battle.net now knows about {}", linked.join(", ")),
        (true, false) => format!("{} — ask Battle.net to locate these", missed.join(", ")),
        (false, false) => format!(
            "Battle.net now knows about {}; ask it to locate {}",
            linked.join(", "),
            missed.join(", ")
        ),
    };
}

//! What the user can do about an error.
//!
//! [`advise`] walks an error's causes while it is still a type and recognises
//! what a person can fix: a full or read-only drive, no connection, a damaged
//! store, a running game, a broken Proton build. Anything else gets nothing.

use std::error::Error;
use std::io::ErrorKind;

use crate::net::NetError;

/// One thing to tell the user, and the button that does it, if there is one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Advice {
    /// A sentence, addressed to the user, saying what to do.
    pub todo: String,
    /// What a front end can do on the user's behalf; the CLI ignores it.
    pub fix: Option<Fix>,
}

/// A remedy a front end can offer as a single press.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fix {
    /// Check the installed game files and repair what is damaged.
    DeepScan,
    /// The Proton settings, to reinstall a build or pick another one.
    ProtonSettings,
    /// The drives settings, to free space or add a drive.
    StorageSettings,
    /// A command for the user to copy and run themselves.
    CopyCommand(String),
    /// Download this Proton build into Lightforge's own folder, which wins
    /// over a copy of the same name elsewhere, and set up again.
    FreshProton(String),
}

impl Advice {
    fn say(todo: impl Into<String>) -> Self {
        Self { todo: todo.into(), fix: None }
    }

    fn with(todo: impl Into<String>, fix: Fix) -> Self {
        Self { todo: todo.into(), fix: Some(fix) }
    }
}

/// What to do about `error`, if it is something the user can do anything
/// about.
///
/// The outermost recognised cause wins; the walk goes all the way down.
pub fn advise(error: &(dyn Error + 'static)) -> Option<Advice> {
    let mut next = Some(error);
    while let Some(error) = next {
        if let Some(advice) = recognise(error) {
            return Some(advice);
        }
        next = transparent(error).or_else(|| error.source());
    }
    None
}

/// The advice for one link of the chain, ignoring what it wraps.
fn recognise(error: &(dyn Error + 'static)) -> Option<Advice> {
    use crate::tact;

    if let Some(e) = error.downcast_ref::<std::io::Error>() {
        return io(e);
    }
    if let Some(e) = error.downcast_ref::<reqwest::Error>() {
        return Some(if e.is_timeout() || e.is_connect() {
            offline()
        } else {
            dropped()
        });
    }
    if let Some(e) = error.downcast_ref::<NetError>() {
        return match e {
            NetError::Status { status, .. } => Some(http(*status)),
            NetError::RateLimited { .. } => Some(Advice::say(
                "GitHub limits how often Proton releases can be checked. Wait up to an hour, \
                 then try again.",
            )),
            NetError::Truncated { .. } => Some(dropped()),
            _ => None,
        };
    }
    if let Some(e) = error.downcast_ref::<tact::cdn::CdnError>() {
        return match e {
            tact::cdn::CdnError::HashMismatch { .. }
            | tact::cdn::CdnError::WrongSize { .. }
            | tact::cdn::CdnError::BadIndex { .. } => {
                Some(Advice::say(
                    "A download server sent a damaged file, and nothing was stored. Try again; \
                     if it keeps happening, the server is out of date and it usually passes \
                     within the hour.",
                ))
            }
            _ => None,
        };
    }
    if let Some(e) = error.downcast_ref::<crate::library::LibraryError>() {
        return match e {
            crate::library::LibraryError::Offline { label } => {
                Some(Advice::say(format!("Mount {label}, then try again.")))
            }
            _ => None,
        };
    }
    if let Some(e) = error.downcast_ref::<crate::preflight::PreflightError>() {
        use crate::preflight::PreflightError as P;
        return match e {
            P::GameRunning(_) | P::OtherBuild(_) => Some(running()),
            P::StillRunning(_) => Some(Advice::say(
                "A Wine process would not stop. Log out and back in, or restart the computer, \
                 then try again.",
            )),
            P::DownloadBusy(_) => {
                Some(Advice::say("Wait for the download to pause, then try again."))
            }
            _ => None,
        };
    }
    if let Some(tact::writer::WriteError::StoreBusy(_)) = error.downcast_ref() {
        return Some(Advice::say(
            "Another Lightforge window or command is changing this game's files. Wait for it \
             to finish, then try again.",
        ));
    }
    if let Some(crate::storage::StorageError::PrefixBusy) = error.downcast_ref() {
        return Some(running());
    }
    if let Some(e) = error.downcast_ref::<tact::idx::IdxError>() {
        use tact::idx::IdxError as I;
        return match e {
            I::Truncated { .. } | I::HeaderHash { .. } | I::DataHash { .. } | I::RaggedSection { .. } => {
                Some(damaged())
            }
            _ => None,
        };
    }
    if let Some(e) = error.downcast_ref::<tact::storage::StorageError>() {
        use tact::storage::StorageError as S;
        return match e {
            S::NoIndices(_) | S::NotBlte { .. } | S::Decode { .. } => Some(damaged()),
            _ => None,
        };
    }
    if let Some(tact::writer::WriteError::BadHeader { .. }) = error.downcast_ref() {
        return Some(damaged());
    }
    if let Some(e) = error.downcast_ref::<crate::proton::ProtonError>() {
        use crate::proton::ProtonError as P;
        return match e {
            P::ChecksumMismatch { .. } | P::EmptyArchive(_) | P::NotAProtonBuild(_) => Some(Advice::with(
                "The Proton download did not check out, so it was not installed. Try again, \
                 or pick another version.",
                Fix::ProtonSettings,
            )),
            _ => None,
        };
    }
    if let Some(crate::prefix::PrefixError::Busy(_)) = error.downcast_ref() {
        return Some(Advice::say(
            "A Wine process would not stop. Log out and back in, or restart the computer, \
             then try again.",
        ));
    }
    if let Some(crate::prefix::PrefixError::MissingRuntime(_)) = error.downcast_ref() {
        return Some(Advice::with(
            "This Proton build is incomplete. Remove it and install it again, or pick another \
             version.",
            Fix::ProtonSettings,
        ));
    }
    if let Some(crate::prefix::PrefixError::MixedBuild { version, .. }) = error.downcast_ref() {
        return Some(Advice::with(
            "This Proton folder has files from an older version mixed in, often from unpacking \
             one over another. Install a fresh copy: Lightforge keeps its own and leaves this \
             folder alone.",
            Fix::FreshProton(version.clone()),
        ));
    }
    if let Some(e) = error.downcast_ref::<crate::runtime::RuntimeError>() {
        use crate::runtime::RuntimeError as R;
        return match e {
            R::NoLoader { .. } | R::WineFailed { .. } | R::NoTls { install: None } => Some(runtime()),
            R::NoTls { install: Some(appid) } => Some(Advice::with(
                "Install the Steam Linux Runtime in Steam: Battle.net runs inside it, since this \
                 system has no 32-bit gnutls of its own. Then try again.",
                Fix::CopyCommand(format!("steam steam://install/{appid}")),
            )),
            _ => None,
        };
    }
    None
}

/// The error a `#[error(transparent)]` variant wraps.
///
/// A transparent variant's `source` is the inner error's own, so walking
/// `source` skips the inner error: `ProtonError::Net(NetError::Status { .. })`
/// has no cause. Every reported wrapper is listed.
fn transparent<'a>(error: &'a (dyn Error + 'static)) -> Option<&'a (dyn Error + 'static)> {
    use crate::tact;

    macro_rules! see_through {
        ($($ty:ty => [$($variant:path),+ $(,)?]),+ $(,)?) => {
            $(
                if let Some(e) = error.downcast_ref::<$ty>() {
                    return match e {
                        $($variant(inner) => Some(inner),)+
                        #[allow(unreachable_patterns)]
                        _ => None,
                    };
                }
            )+
        };
    }

    use crate::addons::catalog::CatalogError;
    use crate::addons::manage::ManageError;
    use crate::battlenet::BattleNetError;
    use crate::error::CoreError;
    use crate::preflight::PreflightError;
    use crate::proton::ProtonError;
    use crate::setup::SetupError;
    use tact::compact::CompactError;
    use tact::download::DownloadError;
    use tact::indices::IndexError;
    use tact::install::InstallError;
    use tact::reclaim::ReclaimError;
    use tact::storage::StorageError;
    use tact::uninstall::UninstallError;
    use tact::verify::VerifyError;
    use tact::version::VersionError;
    use tact::writer::WriteError;

    see_through! {
        CoreError => [CoreError::Net, CoreError::Proton, CoreError::Prefix, CoreError::BattleNet, CoreError::Preflight],
        SetupError => [SetupError::Net, SetupError::Proton, SetupError::Prefix, SetupError::Runtime, SetupError::BattleNet],
        ProtonError => [ProtonError::Net],
        BattleNetError => [BattleNetError::Prefix, BattleNetError::Net],
        PreflightError => [PreflightError::Prefix, PreflightError::BattleNet],
        InstallError => [
            InstallError::Version, InstallError::Cdn, InstallError::Indices,
            InstallError::Download, InstallError::Writer,
        ],
        DownloadError => [DownloadError::Write, DownloadError::Cdn],
        IndexError => [IndexError::Cdn],
        VersionError => [VersionError::Net],
        WriteError => [WriteError::Idx],
        StorageError => [StorageError::Idx],
        VerifyError => [VerifyError::Index, VerifyError::Storage, VerifyError::Write],
        CompactError => [CompactError::Storage, CompactError::Write, CompactError::Verify],
        ReclaimError => [ReclaimError::Storage, ReclaimError::Verify, ReclaimError::Compact],
        UninstallError => [UninstallError::Write],
        ManageError => [ManageError::Catalog, ManageError::Net],
        CatalogError => [CatalogError::Net],
    }
    None
}

fn io(error: &std::io::Error) -> Option<Advice> {
    match error.kind() {
        ErrorKind::StorageFull | ErrorKind::QuotaExceeded => Some(Advice::with(
            "The drive is full. Free some space on it, or put the game on another drive.",
            Fix::StorageSettings,
        )),
        ErrorKind::ReadOnlyFilesystem => Some(Advice::say(
            "That drive is mounted read-only. Mount it read-write, then try again.",
        )),
        ErrorKind::PermissionDenied => Some(Advice::say(
            "Lightforge is not allowed to write there. That folder should belong to you, not \
             to root: check its owner, then try again.",
        )),
        ErrorKind::TimedOut
        | ErrorKind::ConnectionRefused
        | ErrorKind::ConnectionReset
        | ErrorKind::ConnectionAborted
        | ErrorKind::HostUnreachable
        | ErrorKind::NetworkUnreachable
        | ErrorKind::NetworkDown => Some(offline()),
        _ => None,
    }
}

fn http(status: u16) -> Advice {
    match status {
        429 => Advice::say("The server asked Lightforge to slow down. Wait a few minutes, then try again."),
        500..=599 => Advice::say(format!(
            "The server is having trouble (HTTP {status}). Try again in a few minutes."
        )),
        _ => Advice::say(format!(
            "The server refused the request (HTTP {status}). Try again later; if it keeps \
             happening, report it."
        )),
    }
}

fn offline() -> Advice {
    Advice::say("Could not reach the server. Check your internet connection, then try again.")
}

fn dropped() -> Advice {
    Advice::say("The connection dropped partway through. Try again; nothing already stored is lost.")
}

fn running() -> Advice {
    Advice::say("Close the game and Battle.net first, then try again.")
}

fn damaged() -> Advice {
    Advice::with(
        "The game files on disk are damaged. Run a Deep Scan to find and repair them.",
        Fix::DeepScan,
    )
}

fn runtime() -> Advice {
    let remedy = crate::runtime::remedy();
    let todo = match &remedy.note {
        Some(note) => format!("Install your distribution's 32-bit Wine package. {note}"),
        None => "Install your distribution's 32-bit Wine package, then try again.".to_owned(),
    };
    Advice { todo, fix: remedy.command.map(Fix::CopyCommand) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tact;

    fn io_error(kind: ErrorKind) -> crate::error::CoreError {
        crate::error::CoreError::Io { path: "/games".into(), source: std::io::Error::from(kind) }
    }

    #[test]
    fn a_full_drive_points_at_the_drives() {
        let advice = advise(&io_error(ErrorKind::StorageFull)).unwrap();
        assert_eq!(advice.fix, Some(Fix::StorageSettings));
    }

    #[test]
    fn an_error_nobody_can_act_on_gets_no_advice() {
        assert_eq!(advise(&io_error(ErrorKind::InvalidData)), None);
    }

    #[test]
    fn a_status_behind_a_transparent_wrapper_is_still_seen() {
        // `ProtonError::Net` forwards `source` to the `NetError`'s own, which
        // is nothing: without the see-through this walk ends empty-handed.
        let error = crate::proton::ProtonError::Net(NetError::Status {
            url: "https://api.github.com".to_owned(),
            status: 503,
        });
        assert!(advise(&error).unwrap().todo.contains("503"));
    }

    #[test]
    fn a_damaged_index_deep_inside_an_install_offers_a_deep_scan() {
        let error = tact::install::InstallError::Writer(tact::writer::WriteError::Idx(
            tact::idx::IdxError::DataHash { expected: 1, found: 2 },
        ));
        assert_eq!(advise(&error).unwrap().fix, Some(Fix::DeepScan));
    }

    #[test]
    fn a_full_drive_under_a_cdn_fetch_is_the_drive_not_the_network() {
        let error = tact::install::InstallError::Download(tact::download::DownloadError::Write(
            tact::writer::WriteError::Io {
                path: "/games/Data/data/data.004".into(),
                source: std::io::Error::from(ErrorKind::StorageFull),
            },
        ));
        assert_eq!(advise(&error).unwrap().fix, Some(Fix::StorageSettings));
    }

    #[test]
    fn a_busy_store_says_who_has_it_however_it_is_wrapped() {
        let busy = || tact::writer::WriteError::StoreBusy("/games/Data/data".into());
        for error in [
            Box::new(tact::compact::CompactError::Write(busy())) as Box<dyn Error>,
            Box::new(tact::verify::VerifyError::Write(busy())),
            Box::new(tact::uninstall::UninstallError::Write(busy())),
            Box::new(tact::install::InstallError::Writer(busy())),
        ] {
            assert!(advise(error.as_ref()).unwrap().todo.starts_with("Another Lightforge"), "{error}");
        }
    }

    #[test]
    fn a_mixed_proton_folder_offers_a_fresh_copy_of_the_same_build() {
        let error = crate::prefix::PrefixError::MixedBuild {
            version: "GE-Proton11-7".to_owned(),
            build: "/steam/compatibilitytools.d/GE-Proton11-7-x86_64".into(),
        };
        let advice = advise(&error).unwrap();
        assert_eq!(advice.fix, Some(Fix::FreshProton("GE-Proton11-7".to_owned())));
    }

    #[test]
    fn a_running_game_says_to_close_it() {
        let error = crate::preflight::PreflightError::GameRunning(Vec::new());
        assert!(advise(&error).unwrap().todo.starts_with("Close the game"));
    }
}

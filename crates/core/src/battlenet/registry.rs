use super::BattleNetError;
use crate::prefix::ProtonRunner;

/// A registry value applied with `reg add`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegValue {
    pub key: &'static str,
    pub name: &'static str,
    pub kind: &'static str,
    pub data: &'static str,
    pub reason: &'static str,
}

impl RegValue {
    /// Arguments for `reg add`, in the order `reg.exe` expects them.
    pub fn args(&self) -> Vec<String> {
        vec![
            "add".to_owned(),
            self.key.to_owned(),
            "/v".to_owned(),
            self.name.to_owned(),
            "/t".to_owned(),
            self.kind.to_owned(),
            "/d".to_owned(),
            self.data.to_owned(),
            "/f".to_owned(),
        ]
    }
}

/// Registry tweaks for the Battle.net client. Through `reg add`, which needs
/// no temp file, no path translation and no guess at `regedit`'s encoding.
pub fn registry_tweaks() -> Vec<RegValue> {
    vec![
        RegValue {
            key: r"HKEY_CURRENT_USER\Software\Wine\DXVA2",
            name: "backend",
            kind: "REG_SZ",
            data: "va",
            reason: "hardware video decode for Battle.net's own video/UI path (VA-API)",
        },
        RegValue {
            key: r"HKEY_CURRENT_USER\Software\Wine\DllOverrides",
            name: "winemenubuilder.exe",
            kind: "REG_SZ",
            data: "",
            reason: "stop the installer writing Windows shortcuts into the desktop's app menu",
        },
        // No `FontSmoothing*`: Proton already smooths in greyscale, and
        // `winetricks fontsmooth=rgb` values make CEF text thin and snapped.
        // No `LogPixels`: 96 is Wine's default, and any other value needs a
        // display scale the prefix cannot read; a wrong one breaks the store.
    ]
}

/// Apply registry tweaks, then shut the session down again.
pub fn apply_registry(runner: &ProtonRunner, values: &[RegValue]) -> Result<(), BattleNetError> {
    for value in values {
        runner.run("reg", value.args())?;
    }
    runner.wineserver_kill()?;
    Ok(())
}

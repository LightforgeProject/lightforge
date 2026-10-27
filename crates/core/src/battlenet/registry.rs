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

/// Registry tweaks that specifically benefit the Battle.net client.
///
/// `reg add` is used rather than a generated `.reg` file: no temp file inside
/// the prefix, no path translation, and no guessing at the encoding `regedit`
/// wants for a given Wine build.
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
        // Deliberately no `FontSmoothing*`: Proton's prefix already smooths
        // with greyscale, and the subpixel values `winetricks fontsmooth=rgb`
        // writes make the CEF storefront's text look pixel-snapped and thin.
        // Deliberately no `LogPixels`: Wine's default of 96 is right at 1x,
        // writing 96 is a no-op, and any other value needs a display scale
        // that cannot be read from inside the prefix. A wrong one makes the
        // storefront unusable, so it stays a thing the user asks for.
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

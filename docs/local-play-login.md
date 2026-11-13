# Saved login from Play

Set `play_via_battle_net = true` in the top level of Lightforge's
`config.toml` while Lightforge is closed. Play then uses the existing
Battle.net hand-off, including preflight, addon waits and game tracking.
Install, update, pause and Stop retain their existing actions. The default
is false; removing the setting restores direct launches.

Battle.net must have a saved sign-in. Also enable
`battle_net_in_background = true` (Seamless Battle.net Mode), so hiding its
window during the hand-off sends it to the tray instead of quitting it.
Keep `battle_net_minimized = true`
to start it minimized when that saved sign-in is available. Authentication
expiry, two-factor challenges and Battle.net outages can still require
interaction. Lightforge does not store your password.

Verification: the GUI tests exercise both Play routes and configuration
persistence. Existing preflight and Battle.net tests cover readiness and
session tracking. The live check is to restart Lightforge, click Play,
and confirm WoW reaches character selection without entering a password.
A package update can replace this locally built binary; the source branch
retains the change for rebuilding.

Run the deterministic behavior evaluation with
`bash crates/launcher-gui/evals/play-login.sh`. All six checks must pass.

If Battle.net marks a current Lightforge installation as Update, open its
WoW page and let that update finish once. Matching build numbers alone do
not mean Battle.net considers an adopted installation playable. Wait until
its state is UpToDate, then retry Play in Lightforge. Do not work around
this by skipping its update guard or falling back to a password prompt.

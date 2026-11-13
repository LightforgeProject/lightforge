#!/usr/bin/env bash
# Deterministic behavior evaluation. No credentials or live game launch.
set -euo pipefail
cd "$(dirname "$0")/../../.."
checks=(
  play_uses_the_configured_login_path
  play_login_preference_survives_saving_and_old_configs_stay_direct
  saved_login_play_does_not_offer_a_loop_when_battle_net_wants_an_update
  a_hand_off_waits_while_the_background_client_signs_in
  a_game_lightforge_manages_is_never_updated_through_the_client
  stop_asks_first_and_play_comes_back_when_the_game_is_gone
)
for check in "${checks[@]}"; do
  cargo test --release --locked -p lightforge-gui "tests::$check" -- --exact
done
printf 'PASS: %s/%s saved-login behavior checks\n' "${#checks[@]}" "${#checks[@]}"

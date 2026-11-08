// A login was accepted, before the player's saved state is loaded: the bridge looks whether the
// world has a file of them (a first join otherwise).
fn discordsrv_login(player: u128) {
    discordsrv_bridge::player_login(player);
}

// A player joined: the join notification, with the join line as the game words it.
// A fake player (a mod's bot with no client) is not announced.
fn discordsrv_logged_in(game: &mut crate::game::Game<'_>, player: u128, name: &str) {
    if game.native_is_player_fake(player) {
        discordsrv_bridge::fake_joined(player);
        return;
    }
    let message = horizon_client::text::Text::translate(
        "multiplayer.player.joined",
        &[horizon_client::text::Text::literal(name.to_string())],
    )
    .plain();
    discordsrv_bridge::player_join(player, name, &message);
}

// A player left: the leave notification, with the leave line as the game words it.
fn discordsrv_logged_out(player: u128, name: &str) {
    // A fake player (a mod's bot) joined unannounced and leaves so.
    if discordsrv_bridge::fake_left(player) {
        return;
    }
    let message = horizon_client::text::Text::translate(
        "multiplayer.player.left",
        &[horizon_client::text::Text::literal(name.to_string())],
    )
    .plain();
    discordsrv_bridge::player_leave(player, name, &message);
}

// The server stops, its players still on: their leave notifications first (Paper's order: the
// players leave before the plugins stop), then the bridge sends what is left, the shutdown
// message and topics, and disconnects (five seconds at most).
fn discordsrv_stop(game: &mut crate::game::Game<'_>, dedicated: bool) {
    if !dedicated {
        return;
    }
    if discordsrv_bridge::chat_running() {
        for p in game.native_server_players() {
            let message = horizon_client::text::Text::translate(
                "multiplayer.player.left",
                &[horizon_client::text::Text::literal(p.name.clone())],
            )
            .plain();
            discordsrv_bridge::player_leave(p.uuid, &p.name, &message);
        }
    }
    discordsrv_bridge::stop();
}

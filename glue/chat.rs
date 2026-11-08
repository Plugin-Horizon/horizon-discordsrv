// A player's chat message, after the chat filter: to the chat channel (unless a mod before this
// one cancelled it).
fn discordsrv_chat(player: u128, name: &str, message: &str, cancelled: bool) {
    if !cancelled {
        discordsrv_bridge::player_chat(player, name, message);
    }
}

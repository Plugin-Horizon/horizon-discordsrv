// `/discord` (and `/discordsrv`) and its subcommands: answered by the bridge, which makes it not
// reach the server's command tree. What the player may run is asked per `discordsrv.*` node
// (`native_has_permission`: a permissions mod's answer, else the operator level against the node's
// default in DiscordSRV's plugin.yml). Without a running bridge the command is left alone.
fn discordsrv_command(
    game: &mut crate::game::Game<'_>,
    player: u128,
    name: &str,
    command: &str,
    canceled: &mut bool,
    replies: &mut Vec<horizon_client::text::Text>,
) {
    let mut words = command.splitn(2, ' ');
    if !words.next().is_some_and(|root| root.eq_ignore_ascii_case("discord") || root.eq_ignore_ascii_case("discordsrv")) {
        return;
    }
    let has = |node: &str, default_level: u8| game.native_has_permission(player, node, default_level);
    if let Some(lines) = discordsrv_bridge::discord_command(player, name, words.next().unwrap_or(""), &has) {
        *canceled = true;
        replies.extend(lines.iter().map(|l| horizon_client::text::Text::legacy(l)));
    }
}

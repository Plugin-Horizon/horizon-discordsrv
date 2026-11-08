// A player died: the death notification. The hook gives the death message's key only, so the
// message is that key in the server's language with the player's name (a killer's name, the
// second argument of keys such as `death.attack.player`, is not known: GAPS.md).
fn discordsrv_died(game: &mut crate::game::Game<'_>, player: u128, cause: Option<&str>) {
    use horizon_client::text::Text;
    if discordsrv_bridge::is_fake(player) {
        return;
    }
    let known = game
        .native_server_players()
        .into_iter()
        .find(|p| p.uuid == player)
        .map(|p| (p.name, p.dimension))
        .or_else(|| discordsrv_bridge::known_player(player).map(|p| (p.name, p.dimension)));
    let Some((name, dimension)) = known else {
        return;
    };
    let render = |key: &str| {
        let text = Text::translate(key, &[Text::literal(name.clone())]).plain();
        (!text.is_empty() && text != key).then_some(text)
    };
    let message = cause.and_then(render).or_else(|| render("death.attack.generic")).unwrap_or_default();
    discordsrv_bridge::player_death(player, &name, &dimension, &message);
}

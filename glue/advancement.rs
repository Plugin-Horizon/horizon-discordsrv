// A player completed an advancement: the advancement notification with its title. Recipes and
// the tabs' root advancements are not announced in game, so not here either; an advancement
// without a title in the language (a data pack's own) is left out, as there is no helper that
// reads its display (`announce_to_chat`, title): GAPS.md.
fn discordsrv_advancement(player: u128, name: &str, advancement: &str) {
    let path = advancement.split_once(':').map_or(advancement, |(_, p)| p);
    if path.starts_with("recipes/") || path.ends_with("/root") {
        return;
    }
    let key = format!("advancements.{}.title", path.replace('/', "."));
    let title = horizon_client::text::Text::translate(&key, &[]).plain();
    if title.is_empty() || title == key {
        return;
    }
    let dimension = discordsrv_bridge::known_player(player).map(|p| p.dimension).unwrap_or_default();
    discordsrv_bridge::player_advancement(player, name, &dimension, &title);
}

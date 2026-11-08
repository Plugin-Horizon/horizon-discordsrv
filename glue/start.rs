// A dedicated server starts: the bridge reads `config/discordsrv/config.toml` and forwards what
// the log hook (glue/log_line.rs) hears.
fn discordsrv_start(dedicated: bool) {
    if !dedicated {
        discordsrv_bridge::log_feed_off();
        return;
    }
    let Ok(config_dir) = crate::game::Game::native_config_dir() else {
        discordsrv_bridge::log_feed_off();
        return;
    };
    discordsrv_bridge::start(&config_dir, format!("Horizon (MC: {})", horizon_core::VANILLA_VERSION));
}

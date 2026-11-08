// Every tick: the TPS for the channel topics, the players every second, and what the bridge
// queued: console commands (the console channel's with their answer), Discord messages for everyone's chat, lines for one player.
fn discordsrv_tick(game: &mut crate::game::Game<'_>) {
    let (rate, mspt) = game.native_tick_stats();
    let tps = if mspt > 0.0 { rate.min((1000.0 / mspt) as f32) } else { rate };
    let work = discordsrv_bridge::tick(tps, || {
        game.native_server_players()
            .into_iter()
            .map(|p| discordsrv_bridge::Player { uuid: p.uuid, name: p.name, dimension: p.dimension })
            .collect()
    });
    for command in work.commands {
        game.native_run_command(&command);
    }
    // Console channel commands: run as the console with the answer kept
    // (native_run_command_captured, hooks-all), which goes back to the channel.
    for command in work.console {
        let result = game.native_run_command_captured(&command);
        let output = result.output.iter().map(|line| line.plain()).collect();
        discordsrv_bridge::console_output(&command, result.ran, output);
    }
    for line in work.broadcasts {
        game.native_broadcast(horizon_client::text::Text::legacy(&line));
    }
    for (uuid, line) in work.tells {
        game.native_tell(uuid, horizon_client::text::Text::legacy(&line));
    }
}

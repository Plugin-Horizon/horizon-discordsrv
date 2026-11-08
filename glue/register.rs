// The `/discord` and `/discordsrv` command trees in the players' command tree (completion), when
// the bridge runs. The command itself is answered in `command.rs`; these usages are the subcommands
// it answers, with the typed arguments DiscordSRV parses. The staff ones are operator-only (level 2),
// as the `discordsrv.*` nodes default to op in DiscordSRV's plugin.yml, so players do not see them.
// A command's permission node is its first usage's: `discordsrv.discord`. (`/discord ?` is answered
// but not in the tree: Horizon's literal words are lower case letters, digits, `_` and `-`.)
fn discordsrv_register(commands: &mut Vec<horizon_client::mod_api::NativeCommand>) {
    if !discordsrv_bridge::is_running() {
        return;
    }
    let usages: [(&str, u8); 19] = [
        ("", 0),
        (" help [command:word]", 0),
        (" link", 0),
        (" link <player:player_name> <discord:text>", 2),
        (" linked", 0),
        (" linked <target:text>", 2),
        (" unlink", 2),
        (" unlink <target:text>", 2),
        (" clearlinked", 2),
        (" clearlinked <target:text>", 2),
        (" broadcast <message:text>", 2),
        (" bcast <message:text>", 2),
        (" resync", 2),
        (" reload", 2),
        (" language [language:word]", 2),
        (" setlanguage [language:word]", 2),
        (" lang [language:word]", 2),
        (" setlang [language:word]", 2),
        (" debugger [action:word] [categories:text]", 2),
    ];

    for root in ["discord", "discordsrv"] {
        for (i, (args, level)) in usages.iter().enumerate() {
            let command = horizon_client::mod_api::NativeCommand::op(&format!("{root}{args}"), *level);
            commands.push(if i == 0 { command.permission("discordsrv.discord") } else { command });
        }
    }
}

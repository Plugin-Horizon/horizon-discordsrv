// Each message written to `logs/latest.log`, on the log's writer thread (hook native_log_line):
// kept for the console channel. Nothing here may block on the server thread.
fn discordsrv_log_line(level: &str, message: &str) {
    discordsrv_bridge::log_line(level, message);
}

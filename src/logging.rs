use env_logger::Builder;
use std::io::Write;

pub fn init_logging(level: Option<log::LevelFilter>) {
    let level = level.unwrap_or(log::LevelFilter::Info);

    Builder::new()
        .format(|buf, record| {
            writeln!(
                buf,
                "[{}][{}] {}",
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
                record.level(),
                record.args()
            )
        })
        .filter_level(level)
        .parse_env("RUST_LOG")
        .init();
}

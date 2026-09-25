use clap::Parser;

use super::Arguments;

#[test]
fn log_level_defaults_to_info_and_accepts_an_override() {
    let default = Arguments::try_parse_from(["frinkworks"]).expect("default arguments");
    assert_eq!(default.log_level, tracing::Level::INFO);

    let overridden = Arguments::try_parse_from(["frinkworks", "--log-level", "debug"])
        .expect("log level override");
    assert_eq!(overridden.log_level, tracing::Level::DEBUG);
}

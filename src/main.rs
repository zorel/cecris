mod game;
mod http;
mod hub;
mod proto;
mod server;
mod shutdown;
mod util;
mod ws;

use std::net::TcpListener;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

#[derive(Debug, PartialEq)]
enum Action {
    Run { bind: String, port: u16 },
    Help,
}

fn parse_port(value: &str) -> Result<u16, String> {
    value
        .parse::<u16>()
        .map_err(|_| format!("invalid port: {value}"))
}

fn parse_args<I>(args: I, mut bind: String, mut port: u16) -> Result<Action, String>
where
    I: IntoIterator<Item = String>,
{
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(Action::Help),
            "-b" | "--bind" => {
                bind = args
                    .next()
                    .ok_or_else(|| "missing value for --bind".to_string())?;
            }
            "-p" | "--port" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --port".to_string())?;
                port = parse_port(&value)?;
            }
            _ if arg.starts_with("--bind=") => {
                bind = arg["--bind=".len()..].to_string();
            }
            _ if arg.starts_with("--port=") => {
                port = parse_port(&arg["--port=".len()..])?;
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    if bind.trim().is_empty() {
        return Err("bind address must not be empty".to_string());
    }
    if port == 0 {
        return Err("port must be between 1 and 65535".to_string());
    }
    Ok(Action::Run { bind, port })
}

fn print_help() {
    println!(
        "cecris {}\n\
         \n\
         Usage: cecris [options]\n\
         \n\
         Options:\n\
         \x20 -b, --bind <ADDR>  listen address (default: 0.0.0.0, env: BIND)\n\
         \x20 -p, --port <PORT>  listen port (default: 8080, env: PORT)\n\
         \x20 -h, --help         show this help\n\
         \n\
         Command-line values override BIND and PORT.",
        env!("CARGO_PKG_VERSION")
    );
}

fn main() {
    let default_bind = std::env::var("BIND").unwrap_or_else(|_| "0.0.0.0".to_string());
    let default_port = std::env::var("PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(8080);

    let (bind, port) = match parse_args(std::env::args().skip(1), default_bind, default_port) {
        Ok(Action::Run { bind, port }) => (bind, port),
        Ok(Action::Help) => {
            print_help();
            return;
        }
        Err(err) => {
            eprintln!("cecris: {err}");
            print_help();
            std::process::exit(2);
        }
    };

    let hub = Arc::new(hub::Hub::new());

    shutdown::install();

    {
        let hub = hub.clone();
        thread::spawn(move || loop {
            thread::sleep(Duration::from_millis(10));
            if hub.tick() {
                println!("cecris: no games in progress — shutting down");
                std::process::exit(0);
            }
        });
    }

    let listener = match TcpListener::bind((bind.as_str(), port)) {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("failed to bind {bind}:{port}: {err}");
            std::process::exit(1);
        }
    };
    println!("cecris server listening on http://{bind}:{port}");

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let hub = hub.clone();
                thread::spawn(move || server::handle(stream, hub));
            }
            Err(_) => continue,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn defaults_are_used_without_flags() {
        let action = parse_args(args(&[]), "0.0.0.0".to_string(), 8080).unwrap();
        assert_eq!(
            action,
            Action::Run {
                bind: "0.0.0.0".to_string(),
                port: 8080
            }
        );
    }

    #[test]
    fn flags_override_defaults() {
        let action = parse_args(
            args(&["--bind", "127.0.0.1", "--port", "9000"]),
            "0.0.0.0".to_string(),
            8080,
        )
        .unwrap();
        assert_eq!(
            action,
            Action::Run {
                bind: "127.0.0.1".to_string(),
                port: 9000
            }
        );
    }

    #[test]
    fn equals_form_is_accepted() {
        let action = parse_args(
            args(&["--bind=10.0.0.5", "--port=7000"]),
            "0.0.0.0".to_string(),
            8080,
        )
        .unwrap();
        assert_eq!(
            action,
            Action::Run {
                bind: "10.0.0.5".to_string(),
                port: 7000
            }
        );
    }

    #[test]
    fn last_value_wins() {
        let action = parse_args(
            args(&["--port", "8000", "-p", "8001", "-b", "127.0.0.1", "--bind=127.0.0.2"]),
            "0.0.0.0".to_string(),
            8080,
        )
        .unwrap();
        assert_eq!(
            action,
            Action::Run {
                bind: "127.0.0.2".to_string(),
                port: 8001
            }
        );
    }

    #[test]
    fn help_is_reported() {
        assert_eq!(
            parse_args(args(&["--help"]), "0.0.0.0".to_string(), 8080).unwrap(),
            Action::Help
        );
    }

    #[test]
    fn invalid_arguments_fail() {
        assert!(parse_args(args(&["--port"]), "0.0.0.0".to_string(), 8080).is_err());
        assert!(parse_args(args(&["--bind"]), "0.0.0.0".to_string(), 8080).is_err());
        assert!(parse_args(args(&["--port", "nope"]), "0.0.0.0".to_string(), 8080).is_err());
        assert!(parse_args(args(&["--port", "70000"]), "0.0.0.0".to_string(), 8080).is_err());
        assert!(parse_args(args(&["--port", "0"]), "0.0.0.0".to_string(), 8080).is_err());
        assert!(parse_args(args(&["--nope"]), "0.0.0.0".to_string(), 8080).is_err());
        assert!(parse_args(args(&["--bind="]), "0.0.0.0".to_string(), 8080).is_err());
    }
}

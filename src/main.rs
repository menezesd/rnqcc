use std::ffi::OsString;

mod cli;
use cli::quote_make_word;

fn dependency_targets_from_args(args: &[OsString]) -> Vec<String> {
    let mut targets = Vec::new();
    let mut index = 0usize;
    while let Some(arg) = args.get(index) {
        let Some(text) = arg.to_str() else {
            index += 1;
            continue;
        };
        match text {
            "--MT" => {
                if let Some(value) = args.get(index + 1).and_then(|value| value.to_str()) {
                    targets.push(value.to_string());
                }
                index += 2;
            }
            "--MQ" => {
                if let Some(value) = args.get(index + 1).and_then(|value| value.to_str()) {
                    targets.push(quote_make_word(value));
                }
                index += 2;
            }
            _ if text.starts_with("--MT=") => {
                targets.push(text["--MT=".len()..].to_string());
                index += 1;
            }
            _ if text.starts_with("--MQ=") => {
                targets.push(quote_make_word(&text["--MQ=".len()..]));
                index += 1;
            }
            _ => index += 1,
        }
    }
    targets
}

const DRIVER_STACK_SIZE: usize = 256 * 1024 * 1024;

fn main() {
    let result = std::thread::Builder::new()
        .name("rnqcc-driver".to_string())
        .stack_size(DRIVER_STACK_SIZE)
        .spawn(cli::real_main)
        .and_then(|handle| {
            handle.join().map_err(|panic| {
                if let Some(message) = panic.downcast_ref::<&'static str>() {
                    std::io::Error::other(*message)
                } else if let Some(message) = panic.downcast_ref::<String>() {
                    std::io::Error::other(message.clone())
                } else {
                    std::io::Error::other("compiler thread panicked")
                }
            })
        });
    let result = match result {
        Ok(result) => result,
        Err(err) => Err(format!("failed to run compiler driver: {err}")),
    };
    if let Err(err) = result {
        eprintln!("rnqcc: {}", err);
        std::process::exit(1);
    }
}

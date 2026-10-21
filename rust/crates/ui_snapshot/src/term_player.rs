//! Prints `TERMPLAY_FILE` to the terminal, then exits with `TERMPLAY_EXIT` or waits until it is killed.

use std::io::Write;

fn main() {
    if let Ok(path) = std::env::var("TERMPLAY_FILE") {
        match std::fs::read(&path) {
            Ok(bytes) => {
                let mut out = std::io::stdout();
                if let Err(error) = out.write_all(&bytes).and_then(|()| out.flush()) {
                    eprintln!("term_player: {error}");
                }
            }
            Err(error) => eprintln!("term_player: {path}: {error}"),
        }
    }
    if let Some(code) = std::env::var("TERMPLAY_EXIT")
        .ok()
        .and_then(|code| code.parse::<i32>().ok())
    {
        std::process::exit(code);
    }
    loop {
        std::thread::park();
    }
}

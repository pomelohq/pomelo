fn main() {
    let args: Vec<String> = std::env::args().collect();
    let code = pom_ptyhost::cli::run(&args).unwrap_or_else(|| {
        eprintln!("usage: pomelo-pty pty <run|attach|kill> ...");
        2
    });
    std::process::exit(code);
}

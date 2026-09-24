fn main() {
    let args: Vec<String> = std::env::args().collect();
    if let Some(code) = pom_ptyhost::cli::run(&args)
        .or_else(|| pom_mcp::run(&args))
        .or_else(|| pom_agent::run(&args))
    {
        std::process::exit(code);
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let mut stdout = std::io::stdout().lock();
    let mut stderr = std::io::stderr().lock();
    std::process::exit(pom_cli::run(&args, &cwd, &mut stdout, &mut stderr));
}

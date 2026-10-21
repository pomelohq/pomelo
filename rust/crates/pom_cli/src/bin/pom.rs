fn main() {
    let args: Vec<String> = std::env::args().collect();
    if let Some(code) = pom_ptyhost::cli::run(&args)
        .or_else(|| pom_mcp::run(&args))
        .or_else(|| pom_agent::run(&args))
        .or_else(|| pom_agent::run_statusline(&args))
    {
        std::process::exit(code);
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    std::process::exit(pom_cli::run_on_std(&args, &cwd));
}

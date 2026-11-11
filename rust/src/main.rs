fn main() {
    if let Err(error) = lyra::cli::run() {
        eprintln!("lyra: {error:#}");
        std::process::exit(1);
    }
}

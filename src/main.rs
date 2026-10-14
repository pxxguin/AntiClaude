fn main() {
    if let Err(error) = anticlaude::run() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

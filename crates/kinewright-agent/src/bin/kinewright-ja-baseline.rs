fn main() {
    let code = match kinewright_agent::ja_harness::cli() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("JA setup NOT RUN: {error}");
            2
        }
    };
    std::process::exit(code);
}

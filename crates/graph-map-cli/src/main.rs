mod cli;
mod common;
mod http;
mod mcp;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(error) = cli::run(&args) {
        eprintln!("graph-map: {}", error.message);
        let code = if error.status == 400 { 2 } else { 1 };
        std::process::exit(code);
    }
}

//! Thin CLI shell: parse args, delegate to the library, exit with the
//! library-decided code (design §2 exit-code table).

use clap::Parser;

fn main() {
    let cli = agent_profile::cli::Cli::parse();
    std::process::exit(agent_profile::cli::run(cli));
}

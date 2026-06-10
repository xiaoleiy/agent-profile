//! Thin CLI shell: parse args, delegate to the library, exit with the
//! library-decided code (design §2 exit-code table).

use clap::Parser;

fn main() {
    install_broken_pipe_handler();
    let cli = agent_profile::cli::Cli::parse();
    std::process::exit(agent_profile::cli::run(cli));
}

/// A consumer closing stdout early (e.g. `agent-profile … | head`) makes the
/// `print!`/`println!` macros panic with a "Broken pipe" message and exit 101
/// — a code outside the documented space (design §2). Map that specific panic
/// to a clean I/O exit (1) with no backtrace; all other panics keep the
/// default behavior.
fn install_broken_pipe_handler() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = info
            .payload()
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| info.payload().downcast_ref::<&str>().copied())
            .unwrap_or("");
        if msg.contains("Broken pipe") || msg.contains("os error 32") {
            std::process::exit(1);
        }
        default_hook(info);
    }));
}

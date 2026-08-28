use std::net::SocketAddr;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "finery", version, about = "Finery TUI and MCP server")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    #[command(about = "Run protocol-only MCP server over stdin/stdout")]
    Mcp,
    #[command(about = "Run TUI and loopback HTTP MCP for development")]
    Dev {
        #[arg(long, default_value = "127.0.0.1:7347", value_parser = parse_loopback)]
        bind: SocketAddr,
    },
    #[command(about = "Run loopback Streamable HTTP MCP in foreground")]
    Serve {
        #[arg(long, default_value = "127.0.0.1:7345", value_parser = parse_loopback)]
        bind: SocketAddr,
    },
}

fn parse_loopback(value: &str) -> Result<SocketAddr, String> {
    let addr = value
        .parse::<SocketAddr>()
        .map_err(|error| error.to_string())?;
    if !addr.ip().is_loopback() {
        return Err("address must be loopback".into());
    }
    Ok(addr)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    match Cli::parse().command {
        None => {
            tuicore::init();
            finery::run()
        }
        Some(Command::Mcp) => finery::run_mcp(),
        Some(Command::Serve { bind }) => finery::run_http(bind),
        Some(Command::Dev { bind }) => {
            tuicore::init();
            finery::run_dev(bind)
        }
    }
}

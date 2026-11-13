use ae_core::{DEFAULT_PORT, Options, Port};
use anyhow::Result;
use clap::Parser;
use std::net::IpAddr;
use std::path::PathBuf;

/// The core of Agent Editor. Start it, then open the interface.
#[derive(Parser)]
#[command(name = "ae-core", version)]
struct Args {
    /// Port to listen on.
    #[arg(long, default_value_t = DEFAULT_PORT, env = "AE_PORT")]
    port: u16,

    /// Address to listen on. Keep 127.0.0.1 unless another device must connect.
    #[arg(long, default_value = "127.0.0.1", env = "AE_HOST")]
    host: IpAddr,

    /// Folder with the built interface. Without it only the API is served.
    #[arg(long, env = "AE_UI_DIR")]
    ui_dir: Option<PathBuf>,

    /// Where projects, tasks and working copies are kept.
    /// Defaults to the system's folder for application data.
    #[arg(long, env = "AE_DATA_DIR")]
    data_dir: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    ae_core::log_to_stdout();
    let args = Args::parse();
    ae_core::run(Options {
        host: args.host,
        port: Port::Exactly(args.port),
        ui_dir: args.ui_dir,
        data_dir: args.data_dir,
        desktop: None,
    })
    .await
}

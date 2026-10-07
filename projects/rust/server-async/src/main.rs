use clap::Parser;
use rm_server_async::{http::with_service,Service};
use std::net::SocketAddr;

#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "127.0.0.1:7878")]
    address: SocketAddr,

    #[arg(
        long,
        default_value_t = 300,
        value_parser = clap::value_parser!(u64).range(1..)
    )]
    token_ttl_seconds: u64,
}

#[rocket::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let service =
        Service::with_token_ttl_seconds(
            args.token_ttl_seconds,
        );

    let app = with_service(service);
    let config = app
        .figment()
        .clone()
        .merge(("address", args.address.ip()))
        .merge(("port", args.address.port()))
        .merge(("log_level", "critical"));
    app.configure(config).launch().await?;
    Ok(())
}

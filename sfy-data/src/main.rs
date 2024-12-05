#[macro_use]
extern crate log;

use argh::FromArgs;
use env_logger::Env;
use std::path::PathBuf;
use std::sync::Arc;
use warp::{
    http::{Method, Uri},
    Filter,
};

#[macro_use]
extern crate eyre;

#[derive(FromArgs)]
/// The Small Friendly Data host.
struct Sfy {
    /// configuration file.
    #[argh(option, short = 'c', default = "PathBuf::from(\"sfy-data.toml\")")]
    config: PathBuf,
}

mod buoys;
mod config;
mod database;

pub struct SfyState {
    pub db: database::Database,
    pub config: config::Config,
}

pub type State = Arc<SfyState>;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    color_eyre::install()?;
    env_logger::Builder::from_env(
        Env::default().default_filter_or("warn,sfy_data=debug,warp=debug"),
    )
    .init();

    info!("sfy-data server");
    let sfy: Sfy = argh::from_env();
    let config = config::Config::from_path(sfy.config);

    let database = config.database.clone().expect("no database path specified");
    let database = database::Database::open(&database).await?;

    let state = Arc::new(SfyState {
        db: database,
        config: config.clone(),
    });

    info!("listening on: {:?}", config.address);

    let cors = warp::cors()
        .allow_any_origin()
        .allow_methods(&[Method::GET, Method::POST])
        .allow_headers(["SFY_AUTH_TOKEN"]);

    if let Some(dir) = config.files {
        info!("serving files in directory: {:?}", dir);
        let redirect = warp::path("sfy")
            .and(warp::path::end())
            .and(warp::path::full())
            .and_then(move |p: warp::path::FullPath| async move {
                if p.as_str().ends_with('/') {
                    Err(warp::reject())
                } else {
                    Ok(warp::redirect(Uri::from_static("/sfy/")))
                }
            });

        let sfy = redirect.or(warp::path("sfy").and(warp::fs::dir(dir)));
        let api = sfy
            .or(buoys::filters(state))
            .with(cors)
            .with(warp::log("sfy_data::api"));
        warp::serve(api).run(config.address).await;
    } else {
        let api = buoys::filters(state)
            .with(cors)
            .with(warp::log("sfy_data::api"));
        warp::serve(api).run(config.address).await;
    };

    Ok(())
}

#[cfg(test)]
async fn test_state() -> State {
    let config = config::Config::test_config();
    let db = database::Database::temporary().await;

    let state = SfyState { config, db };
    let state = Arc::new(state);

    state
}

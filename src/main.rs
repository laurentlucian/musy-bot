mod config;
mod logging;

use std::env;

use chrono::Utc;
use config::Config;
use log::*;
use serenity::async_trait;
use serenity::framework::standard::macros::{command, group};
use serenity::framework::standard::{CommandResult, StandardFramework};
use serenity::model::channel::Message;
use serenity::prelude::*;
use serenity::Client;

#[group]
#[commands(ping, latency)]
struct General;

struct Handler;

#[async_trait]
impl EventHandler for Handler {
    async fn message(&self, _context: Context, msg: Message) {
        debug!("msg content {:?}", msg.content);
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::load()?;
    logging::setup_logging(&config)?;

    debug!("{:#?}", config);

    let framework = StandardFramework::new()
        // set the bot's prefix to "~"
        .configure(|c| c.prefix("~"))
        .group(&GENERAL_GROUP);

    // Login with a bot token from the environment
    let token = env::var("DISCORD_TOKEN").expect("token");
    let mut client = Client::builder(token)
        .event_handler(Handler)
        .framework(framework)
        .await
        .expect("Error creating client");

    info!("Starting client...");
    let http = client.cache_and_http.http.clone();

    let appinfo = http.get_current_application_info().await?;

    debug!("{:#?}", appinfo);
    info!(
        "Connected with application {} ({}). Owned by {} ({})",
        appinfo.name,
        appinfo.id,
        appinfo.owner.tag(),
        appinfo.owner.id
    );

    match client.start_autosharded().await {
        Ok(_) => info!("Client shut down succesfully!"),
        Err(e) => error!("Client returned error: {}", e),
    }

    Ok(())
}

#[command]
async fn ping(ctx: &Context, msg: &Message) -> CommandResult {
    msg.reply(ctx, "Pong!").await?;

    Ok(())
}

#[command]
async fn latency(ctx: &Context, msg: &Message) -> CommandResult {
    let delay = (Utc::now() - msg.timestamp).num_milliseconds();

    msg.reply(ctx, &format!("The latency is {:?}ms", delay))
        .await?;

    Ok(())
}

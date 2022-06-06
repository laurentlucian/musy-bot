mod log_config;
mod logging;

use chrono::Utc;
use log::*;
use log_config::Config;
use std::env;

// This trait adds the `register_songbird` and `register_songbird_with` methods to the client builder below.
// The voice client can be retrieved in any command using `songbird::get(ctx).await`.
use songbird::SerenityInit;

use serenity::{
  async_trait,
  client::{Client, Context, EventHandler},
  framework::{
    standard::{
      macros::{command, group},
      CommandResult,
    },
    StandardFramework,
  },
  model::{channel::Message, gateway::Ready},
  Result,
};

#[group]
#[commands(join, leave, ping, latency)]
struct General;

struct Handler;

#[async_trait]
impl EventHandler for Handler {
  async fn ready(&self, _: Context, ready: Ready) {
    debug!("{} is connected!", ready.user.name);
  }

  async fn message(&self, _context: Context, msg: Message) {
    debug!("msg content {:?}", msg.content);
  }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
  let config = Config::load()?;
  logging::setup_logging(&config)?;

  // Login with a bot token from the environment
  let token = env::var("DISCORD_TOKEN").expect("token:");
  let framework = StandardFramework::new()
    // set the bot's prefix to "~"
    .configure(|c| c.prefix("~"))
    .group(&GENERAL_GROUP);

  info!("Starting client...");
  let mut client = Client::builder(token)
    .event_handler(Handler)
    .framework(framework)
    .register_songbird()
    .await
    .expect("Error creating client");

  let http = client.cache_and_http.http.clone();
  let appinfo = http.get_current_application_info().await?;

  debug!("{:#?}", appinfo);
  info!(
    "Connected with {} ({}). Owned by {} ({})",
    appinfo.name,
    appinfo.id,
    appinfo.owner.tag(),
    appinfo.owner.id
  );

  let shard_manager = client.shard_manager.clone();

  tokio::spawn(async move {
    let _ = client
      .start()
      .await
      .map_err(|why| println!("Client ended: {:?}", why));
  });

  // waits for signal to continue further
  tokio::signal::ctrl_c().await?;
  println!("Received Ctrl-C, shutting down.");
  // leaves voice channel once continued
  shard_manager.lock().await.shutdown_all().await;

  Ok(())
}

#[command]
#[only_in(guilds)]
async fn join(ctx: &Context, msg: &Message) -> CommandResult {
  let guild = msg.guild(&ctx.cache).await.unwrap();
  let guild_id = guild.id;

  let channel_id = guild
    .voice_states
    .get(&msg.author.id)
    .and_then(|voice_state| voice_state.channel_id);

  let connect_to = match channel_id {
    Some(channel) => channel,
    None => {
      check_msg(msg.reply(ctx, "Not in a voice channel").await);

      return Ok(());
    }
  };

  let manager = songbird::get(ctx)
    .await
    .expect("Songbird Client failed to initialize.")
    .clone();

  let _handler = manager.join(guild_id, connect_to).await;

  Ok(())
}

#[command]
#[only_in(guilds)]
async fn leave(ctx: &Context, msg: &Message) -> CommandResult {
  let guild = msg.guild(&ctx.cache).await.unwrap();
  let guild_id = guild.id;

  let manager = songbird::get(ctx)
    .await
    .expect("Songbird Client failed to initialize.")
    .clone();
  let has_handler = manager.get(guild_id).is_some();

  if has_handler {
    if let Err(e) = manager.remove(guild_id).await {
      check_msg(
        msg
          .channel_id
          .say(&ctx.http, format!("Failed: {:?}", e))
          .await,
      );
    }

    check_msg(msg.channel_id.say(&ctx.http, "Left voice channel").await);
  } else {
    check_msg(msg.reply(ctx, "Not in a voice channel").await);
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

  msg
    .reply(ctx, &format!("The latency is {:?}ms", delay))
    .await?;

  Ok(())
}

/// checks that a message successfully sent; if not, then logs why.
fn check_msg(result: Result<Message>) {
  if let Err(why) = result {
    debug!("Error sending message: {:?}", why);
  }
}

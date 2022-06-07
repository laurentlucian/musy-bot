mod log_config;
mod logging;

use chrono::Utc;
use log::*;
use log_config::Config;
use std::{env, fmt::Debug};

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

use hyper::{body::Buf, header, Body, Client as HyperClient, Request};
use hyper_tls::HttpsConnector;
use serde_derive::{Deserialize, Serialize};

#[group]
#[commands(join, leave, ping, latency)]
struct General;

struct Handler;

#[derive(Debug, Deserialize)]
struct OAIChoices {
  text: String,
  index: u8,
  logprobs: Option<u8>,
  finish_reason: String,
}

#[derive(Debug, Deserialize)]
struct OAIResponse {
  id: Option<String>,
  object: Option<String>,
  created: Option<u64>,
  model: Option<String>,
  choices: Vec<OAIChoices>,
}

#[derive(Debug, Serialize)]
struct OAIRequest {
  prompt: String,
  temperature: f32,
  top_p: u8,
  frequency_penalty: u8,
  presence_penalty: f32,
  max_tokens: u32,
}

#[async_trait]
impl EventHandler for Handler {
  async fn ready(&self, _: Context, ready: Ready) {
    debug!("{} is connected!", ready.user.name);
  }

  async fn message(&self, ctx: Context, msg: Message) {
    if msg.author.bot {
      debug!("msg from bot ignored");
      return;
    }
    if msg.content.starts_with("!") {
      debug!("command ignored");
      return;
    }

    let channel_id = msg.channel_id.0;

    if channel_id != 983513540306534400 {
      debug!("channel ignored");
      return;
    }

    ctx.http.broadcast_typing(channel_id).await.unwrap();

    let https = HttpsConnector::new();
    let client = HyperClient::builder().build(https);
    let uri = "https://api.openai.com/v1/engines/text-davinci-002/completions";

    let oai_token = env::var("OAI_TOKEN").expect("token:");
    let auth_header_val = format!("Bearer {}", oai_token);
    println!("{}", auth_header_val);

    // preamble can send conversation history and AI would respond with local context.
    // eg: \n\nHuman: Hello, who are you?\nAI: I am an AI created by OpenAI. How can I help you today?
    let preamble = "The following is a conversation with an AI assistant. The assistant is helpful, creative, clever, funny and very friendly.";
    let message = msg.content.clone();

    let oai_request = OAIRequest {
      prompt: format!("{} {}", preamble, message),
      temperature: 0.9,
      max_tokens: 150,
      top_p: 1,
      frequency_penalty: 0,
      presence_penalty: 0.6,
    };

    let body = Body::from(serde_json::to_vec(&oai_request).unwrap());

    let req = Request::post(uri)
      .header(header::CONTENT_TYPE, "application/json")
      .header("Authorization", &auth_header_val)
      .body(body)
      .unwrap();

    let res = client.request(req).await.unwrap();

    let body = hyper::body::aggregate(res).await.unwrap();

    let json: OAIResponse = serde_json::from_reader(body.reader()).unwrap();
    println!("{:?}", json);

    let delay = (Utc::now() - msg.timestamp).num_milliseconds();

    check_msg(
      msg
        .channel_id
        .say(
          &ctx.http,
          format!(
            "{} `{}ms delay`",
            json.choices[0].text.to_lowercase(),
            delay
          ),
        )
        .await,
    );

    debug!("msg.content {:?}", msg.content);
  }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
  let config = Config::load()?;
  logging::setup_logging(&config)?;

  // Login with a bot token from the environment
  let token = env::var("DISCORD_TOKEN").expect("token:");
  let framework = StandardFramework::new()
    // set the bot's prefix to "!"
    .configure(|c| c.prefix("!"))
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

  // debug!("{:#?}", appinfo);
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
      check_msg(msg.reply(ctx, "`join a voice channel first`").await);

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

    check_msg(msg.channel_id.say(&ctx.http, "`left voice channel`").await);
  } else {
    check_msg(msg.reply(ctx, "`not on voice`").await);
  }

  Ok(())
}

#[command]
async fn ping(ctx: &Context, msg: &Message) -> CommandResult {
  msg.reply(ctx, "`pong`").await?;

  Ok(())
}

#[command]
async fn latency(ctx: &Context, msg: &Message) -> CommandResult {
  let delay = (Utc::now() - msg.timestamp).num_milliseconds();

  msg
    .reply(ctx, &format!("`the latency is {:?}ms`", delay))
    .await?;

  Ok(())
}

/// checks that a message successfully sent; if not, then logs why.
fn check_msg(result: Result<Message>) {
  if let Err(why) = result {
    debug!("Error sending message: {:?}", why);
  }
}

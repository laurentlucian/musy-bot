mod log_config;
mod logging;

use chrono::Utc;
use log::*;
use log_config::Config;
use std::{env, fmt::Debug, sync::Arc};

// This trait adds the `register_songbird` and `register_songbird_with` methods to the client builder below.
// The voice client can be retrieved in any command using `songbird::get(ctx).await`.
use songbird::input;
use songbird::SerenityInit;

mod lib {
  pub mod player;
}

use lib::player::{SpotifyPlayer, SpotifyPlayerKey};
use librespot::core::mercury::MercuryError;
use librespot::playback::config::Bitrate;
use librespot::playback::player::PlayerEvent;
use tokio::sync::Mutex;
use tokio::time::{sleep, Duration};

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
  model::{
    channel::Message, gateway, gateway::Ready, id, prelude::Activity, user, voice::VoiceState,
  },
  prelude::TypeMapKey,
  Result as SerenityResult,
};

use hyper::{body::Buf, header, Body, Client as HyperClient, Request};
use hyper_tls::HttpsConnector;
use serde_derive::{Deserialize, Serialize};

#[group]
#[commands(join, leave, ping, latency)]
struct General;

struct Handler;

pub struct UserIdKey;
impl TypeMapKey for UserIdKey {
  type Value = id::UserId;
}

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
    debug!("{} is connected.", ready.user.name);
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

    // preamble can send conversation history and AI would respond with local context.
    // eg: \n\nHuman: Hello, who are you?\nAI: I am an AI created by OpenAI. How can I help you today?
    let preamble = "The following is a conversation with an AI assistant. The assistant is helpful, creative, clever and mean.";
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

  async fn cache_ready(&self, ctx: Context, guilds: Vec<id::GuildId>) {
    let guild_id = match guilds.first() {
      Some(guild_id) => *guild_id,
      None => {
        panic!("Not currently in any guilds.");
      }
    };

    let data = ctx.data.read().await;

    let player = data.get::<SpotifyPlayerKey>().unwrap().clone();
    let user_id = *data
      .get::<UserIdKey>()
      .expect("User ID placed in at initialisation.");

    // Handle case when user is in VC when bot starts
    let guild = ctx
      .cache
      .guild(guild_id)
      .await
      .expect("Could not find guild in cache.");

    let channel_id = guild
      .voice_states
      .get(&user_id)
      .and_then(|voice_state| voice_state.channel_id);
    drop(guild);

    if channel_id.is_some() {
      // Enable casting
      player.lock().await.enable_connect().await;
    }

    let c = ctx.clone();

    // Handle Spotify events
    tokio::spawn(async move {
      loop {
        let channel = player.lock().await.event_channel.clone().unwrap();
        let mut receiver = channel.lock().await;

        let event = match receiver.recv().await {
          Some(e) => e,
          None => {
            // Busy waiting bad but quick and easy
            sleep(Duration::from_millis(256)).await;
            continue;
          }
        };

        match event {
          PlayerEvent::Stopped { .. } => {
            c.set_presence(None, user::OnlineStatus::Online).await;

            let manager = songbird::get(&c)
              .await
              .expect("Songbird Voice client placed in at initialisation.")
              .clone();

            let _ = manager.remove(guild_id).await;
          }

          PlayerEvent::Started { .. } => {
            let manager = songbird::get(&c)
              .await
              .expect("Songbird Voice client placed in at initialisation.")
              .clone();

            let guild = c
              .cache
              .guild(guild_id)
              .await
              .expect("Could not find guild in cache.");

            let channel_id = match guild
              .voice_states
              .get(&user_id)
              .and_then(|voice_state| voice_state.channel_id)
            {
              Some(channel_id) => channel_id,
              None => {
                println!("Could not find user in VC.");
                continue;
              }
            };

            let _handler = manager.join(guild_id, channel_id).await;

            if let Some(handler_lock) = manager.get(guild_id) {
              let mut handler = handler_lock.lock().await;

              let mut decoder = input::codec::OpusDecoderState::new().unwrap();
              decoder.allow_passthrough = false;

              let source = input::Input::new(
                true,
                input::reader::Reader::Extension(Box::new(
                  player.lock().await.emitted_sink.clone(),
                )),
                input::codec::Codec::FloatPcm,
                input::Container::Raw,
                None,
              );

              handler.set_bitrate(songbird::driver::Bitrate::Auto);

              handler.play_only_source(source);
            } else {
              println!("Could not fetch guild by ID.");
            }
          }

          PlayerEvent::Paused { .. } => {
            c.set_presence(None, user::OnlineStatus::Online).await;
          }

          PlayerEvent::Playing { track_id, .. } => {
            let track: Result<librespot::metadata::Track, MercuryError> =
              librespot::metadata::Metadata::get(&player.lock().await.session, track_id).await;

            if let Ok(track) = track {
              let artist: Result<librespot::metadata::Artist, MercuryError> =
                librespot::metadata::Metadata::get(
                  &player.lock().await.session,
                  *track.artists.first().unwrap(),
                )
                .await;

              if let Ok(artist) = artist {
                let listening_to = format!("{}: {}", artist.name, track.name);

                c.set_presence(
                  Some(gateway::Activity::listening(listening_to)),
                  user::OnlineStatus::Online,
                )
                .await;
              }
            }
          }

          _ => {}
        }
      }
    });
  }

  async fn voice_state_update(
    &self,
    ctx: Context,
    _: Option<id::GuildId>,
    old: Option<VoiceState>,
    new: VoiceState,
  ) {
    let data = ctx.data.read().await;

    let user_id = data.get::<UserIdKey>();

    if new.user_id.to_string() != user_id.unwrap().to_string() {
      return;
    }

    let player = data.get::<SpotifyPlayerKey>().unwrap();

    let guild = ctx
      .cache
      .guild(ctx.cache.guilds().await.first().unwrap())
      .await
      .unwrap();

    // If user just connected
    if old.clone().is_none() {
      // Enable casting
      ctx.set_presence(None, user::OnlineStatus::Online).await;
      player.lock().await.enable_connect().await;
      return;
    }

    // If user disconnected
    if old.clone().unwrap().channel_id.is_some() && new.channel_id.is_none() {
      // Disable casting
      ctx.invisible().await;
      player.lock().await.disable_connect().await;

      // Disconnect
      let manager = songbird::get(&ctx)
        .await
        .expect("Songbird Voice client placed in at initialisation.")
        .clone();

      let _handler = manager.remove(guild.id).await;

      return;
    }

    // If user moved channels
    if old.unwrap().channel_id.unwrap() != new.channel_id.unwrap() {
      let bot_id = ctx.cache.current_user_id().await;

      let bot_channel = guild
        .voice_states
        .get(&bot_id)
        .and_then(|voice_state| voice_state.channel_id);

      if Option::is_some(&bot_channel) {
        let manager = songbird::get(&ctx)
          .await
          .expect("Songbird Voice client placed in at initialisation.")
          .clone();

        if let Some(guild_id) = ctx.cache.guilds().await.first() {
          let _handler = manager.join(*guild_id, new.channel_id.unwrap()).await;
        }
      }

      return;
    }
  }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
  let config = Config::load()?;
  logging::setup_logging(&config)?;

  // tracing_subscriber::fmt::init();

  let username =
    env::var("SPOTIFY_USERNAME").expect("Expected a Spotify username in the environment");
  let password =
    env::var("SPOTIFY_PASSWORD").expect("Expected a Spotify password in the environment");
  let user_id = env::var("DISCORD_USER_ID").expect("Expected a Discord user ID in the environment");

  let mut cache_dir = None;
  if let Ok(c) = env::var("CACHE_DIR") {
    cache_dir = Some(c);
  }
  let player = Arc::new(Mutex::new(
    SpotifyPlayer::new(username, password, Bitrate::Bitrate320, cache_dir).await,
  ));

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
    .type_map_insert::<SpotifyPlayerKey>(player)
    .type_map_insert::<UserIdKey>(id::UserId::from(user_id.parse::<u64>().unwrap()))
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
fn check_msg(result: SerenityResult<Message>) {
  if let Err(why) = result {
    debug!("Error sending message: {:?}", why);
  }
}

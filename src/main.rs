use anyhow::Context as _;
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use clear::{clear, clear_all, clear_channel, clear_user};
use datetime::parse_time;
use poise::{
    Context, CreateReply,
    serenity_prelude::{
        CacheHttp, ClientBuilder, ComponentInteraction, ComponentInteractionData,
        ComponentInteractionDataKind, CreateActionRow, CreateButton, CreateInteractionResponse,
        CreateInteractionResponseFollowup, CreateInteractionResponseMessage, CreateMessage,
        DiscordJsonError, EditInteractionResponse, EditMessage, ErrorResponse, FullEvent,
        GatewayIntents, GuildId, Interaction, UserId,
    },
};
use rand::seq::IteratorRandom;
use redb::{Database, ReadableTable, TableDefinition};
use std::{cmp::min, collections::HashSet, sync::Arc, time::Duration};
use structs::{Giveaway, GiveawayId, GuildState, MyHttpCache, RealGiveaway, UserAction};

#[path = "bincode.rs"]
mod bc;
mod clear;
mod datetime;
mod structs;

pub(crate) const TOKEN: &str = include_str!("../token");
pub(crate) const DATABASE_PATH: &str = "db.redb";
pub(crate) const TABLE: TableDefinition<u64, bc::Bincode<GuildState>> =
    TableDefinition::new("guilds");

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("Starting...");
    let mut db = Database::create(DATABASE_PATH)?;
    db.compact()?;
    {
        let w = db.begin_write()?;
        let t = w.open_table(TABLE)?;
        drop(t);
        w.commit()?;
    }
    let db = Arc::new(db);
    dump_db(&db);

    let framework = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            commands: vec![create(), timezone(), info(), clear(), clear_all()],
            event_handler: |ctx, event, framework, data| {
                Box::pin(event_handler(ctx, event, framework, data))
            },
            ..Default::default()
        })
        .setup(|ctx, _ready, framework| {
            Box::pin(async move {
                poise::builtins::register_globally(ctx, &framework.options().commands).await?;

                let http = MyHttpCache::new(ctx.http.clone(), ctx.cache.clone());
                {
                    let db_read = db.begin_read()?;
                    let table = db_read.open_table(TABLE)?;
                    let mut iter = table.iter()?;
                    while let Some(Ok(guild)) = iter.next() {
                        let guild_id = GuildId::from(guild.0.value());
                        let guild = guild.1.value();
                        for giveaway in guild.giveaways {
                            let giveaway_id = giveaway.0;
                            let giveaway: RealGiveaway = giveaway.1.into();
                            let db = db.clone();
                            let http = http.clone();
                            if let Some(time) = giveaway.time {
                                tokio::spawn(async move {
                                    finish_task(guild_id, giveaway_id, time, db, http)
                                        .await
                                        .unwrap();
                                });
                            }
                        }
                    }
                }

                println!("Prepared and connected to disord");
                Ok(db)
            })
        })
        .build();
    let client = ClientBuilder::new(TOKEN, GatewayIntents::non_privileged())
        .framework(framework)
        .await;
    client?.start().await?;

    Ok(())
}

async fn event_handler(
    ctx: &poise::serenity_prelude::Context,
    event: &poise::serenity_prelude::FullEvent,
    _framework: poise::FrameworkContext<'_, Arc<Database>, anyhow::Error>,
    db: &Arc<Database>,
) -> anyhow::Result<()> {
    match event {
        FullEvent::MessageDelete {
            channel_id: channel,
            deleted_message_id: message,
            guild_id: Some(guild),
        } => {
            let data: Option<(GiveawayId, RealGiveaway)> = db_write(db, *guild, move |state| {
                state
                    .giveaways
                    .iter()
                    .find(|(_, ga)| ga.channel == channel.get() && ga.message == message.get())
                    .map(|(id, _)| *id)
                    .and_then(|id| state.giveaways.remove(&id).map(|ga| (id, ga)))
            })?
            .map(|(a, b)| (a, b.into()));
            if let Some((id, giveaway)) = data {
                if let Err(err) = cancel_giveaway(&giveaway, &ctx).await {
                    eprintln!("Error cancelling giveaway: {}", err);
                    let giveaway: Giveaway = giveaway.into();
                    db_write(db, *guild, move |state| {
                        state.giveaways.insert(id, giveaway)
                    })?;
                }
            }
        }
        FullEvent::InteractionCreate {
            interaction: Interaction::Component(interaction),
        } => {
            interaction.defer(&ctx).await?;
            match &interaction {
                ComponentInteraction {
                    guild_id: Some(guild),
                    member: Some(member),
                    user,
                    data:
                        ComponentInteractionData {
                            custom_id,
                            kind: ComponentInteractionDataKind::Button,
                            ..
                        },
                    ..
                } => {
                    let action: UserAction = serde_json::from_str(&custom_id)?;
                    match action {
                        UserAction::Add(id) => {
                            add_user(*guild, id, user.id, db).await?;
                            interaction
                                .create_followup(
                                    &ctx,
                                    CreateInteractionResponseFollowup::new()
                                        .content("Du nimmst am Giveaway teil")
                                        .ephemeral(true),
                                )
                                .await?;
                        }
                        UserAction::Remove(id) => {
                            remove_user(*guild, id, user.id, db).await?;
                            interaction
                                .create_followup(
                                    &ctx,
                                    CreateInteractionResponseFollowup::new()
                                        .content("Du nimmst nicht mehr am Giveaway teil")
                                        .ephemeral(true),
                                )
                                .await?;
                        }
                        UserAction::Finish(id)
                            if member.permissions.is_some_and(|p| p.create_events()) =>
                        {
                            let giveaway: Option<RealGiveaway> =
                                db_write(db, *guild, move |state| state.giveaways.remove(&id))?
                                    .map(|v| v.into());
                            if let Some(giveaway) = giveaway {
                                if let Err(err) = finish_giveaway(&giveaway, &ctx).await {
                                    eprintln!("Error finishing giveaway: {}", err);
                                    let giveaway: Giveaway = giveaway.into();
                                    db_write(db, *guild, move |state| {
                                        state.giveaways.insert(id, giveaway)
                                    })?;
                                }
                            }
                        }
                        UserAction::Cancel(id)
                            if member.permissions.is_some_and(|p| p.create_events()) =>
                        {
                            let giveaway: Option<RealGiveaway> =
                                db_write(db, *guild, |state| state.giveaways.remove(&id))?
                                    .map(|v| v.into());
                            if let Some(giveaway) = giveaway {
                                if let Err(err) = cancel_giveaway(&giveaway, &ctx).await {
                                    eprintln!("Error cancelling giveaway: {}", err);
                                    let giveaway: Giveaway = giveaway.into();
                                    db_write(db, *guild, move |state| {
                                        state.giveaways.insert(id, giveaway);
                                    })?;
                                }
                            }
                        }
                        UserAction::Clear(None) => {
                            interaction.message.delete(&ctx).await?;
                        }
                        UserAction::ClearAll(None) => {
                            interaction.message.delete(&ctx).await?;
                        }
                        UserAction::Clear(Some((guild, user)))
                            if member.permissions.is_some_and(|p| p.manage_channels()) =>
                        {
                            interaction
                                .edit_response(
                                    &ctx,
                                    EditInteractionResponse::new()
                                        .content("Das dauert einen kleinen Moment...")
                                        .components(Vec::new()),
                                )
                                .await?;
                            let count = clear_user(&ctx, guild, user).await?;
                            interaction
                                .create_followup(
                                    &ctx,
                                    CreateInteractionResponseFollowup::new()
                                        .content(format!(
                                            "Es wurden {count} Nachrichten von <@{user}> gelöscht"
                                        ))
                                        .ephemeral(false),
                                )
                                .await?;
                            interaction.delete_response(&ctx).await?;
                        }
                        UserAction::ClearAll(Some(channel))
                            if member.permissions.is_some_and(|p| p.manage_channels()) =>
                        {
                            interaction
                                .edit_response(
                                    &ctx,
                                    EditInteractionResponse::new()
                                        .content("Das dauert einen kleinen Moment...")
                                        .components(Vec::new()),
                                )
                                .await?;
                            clear_channel(&ctx, channel).await?;
                            interaction.delete_response(&ctx).await?;
                            channel
                                .send_message(
                                    &ctx,
                                    CreateMessage::new().content("_Kanal wurde geleert_"),
                                )
                                .await?;
                        }
                        _ => {
                            interaction.delete_response(&ctx).await?;
                            interaction
                                .create_response(
                                    ctx,
                                    CreateInteractionResponse::Message(
                                        CreateInteractionResponseMessage::new()
                                            .content("Keine Berechtigung")
                                            .ephemeral(true),
                                    ),
                                )
                                .await?;
                        }
                    }
                }
                _ => {}
            }
            //interaction
            //    .create_followup(&ctx, CreateInteractionResponseFollowup::new())
            //    .await?;
        }
        _ => {}
    }
    Ok(())
}

async fn add_user(
    guild: GuildId,
    id: GiveawayId,
    user: UserId,
    db: &Database,
) -> anyhow::Result<bool> {
    let success = db_write(db, guild, move |state| {
        state
            .giveaways
            .get_mut(&id)
            .map(|giveaway| giveaway.participants.insert(user.get()))
            .unwrap_or(false)
    })?;
    Ok(success)
}

//  Returns true, if the user was removed and false, if the user wasn't a participant
async fn remove_user(
    guild: GuildId,
    id: GiveawayId,
    user: UserId,
    db: &Database,
) -> anyhow::Result<bool> {
    let success = db_write(db, guild, move |state| {
        state
            .giveaways
            .get_mut(&id)
            .map(|giveaway| giveaway.participants.remove(&user.get()))
            .unwrap_or(false)
    })?;
    Ok(success)
}

async fn finish_task(
    guild: GuildId,
    id: GiveawayId,
    time: DateTime<Utc>,
    db: Arc<Database>,
    http: impl CacheHttp,
) -> anyhow::Result<()> {
    let now = chrono::Utc::now();
    let diff = time.timestamp() - now.timestamp();
    if diff > 0 {
        tokio::time::sleep(Duration::from_secs(diff as u64)).await;
    }
    let giveaway: Option<RealGiveaway> =
        db_write(&db, guild, move |state| state.giveaways.remove(&id))?.map(|v| v.into());
    if let Some(giveaway) = giveaway
        && giveaway.time.as_ref().is_some_and(|dt| dt == &time)
    {
        if let Err(err) = finish_giveaway(&giveaway, &http).await {
            eprintln!("Error finishing giveaway: {}", err);
            let giveaway: Giveaway = giveaway.into();
            db_write(&db, guild, move |state| {
                state.giveaways.insert(id, giveaway)
            })?;
        }
    }
    Ok(())
}

async fn finish_giveaway(giveaway: &RealGiveaway, http: &impl CacheHttp) -> anyhow::Result<()> {
    let winners_count = min(giveaway.winners as usize, giveaway.participants.len());
    let mut winners: HashSet<UserId> = HashSet::with_capacity(winners_count);
    while winners.len() < winners_count {
        winners.insert(
            *giveaway
                .participants
                .iter()
                .choose(&mut rand::rng())
                .unwrap(),
        );
    }
    let mut winners_str = "Gewinner:".to_string();
    for (i, winner) in winners.into_iter().enumerate() {
        winners_str.push_str(&format!("\n{}. <@{winner}>", i + 1));
    }
    if winners_count == 0 {
        winners_str = "Keine Teilnehmer".to_string();
    }
    giveaway
        .channel
        .edit_message(
            http,
            giveaway.message,
            EditMessage::new()
                .content(giveaway.get_message(true))
                .components(Vec::new()),
        )
        .await?;
    giveaway
        .channel
        .send_message(
            http,
            CreateMessage::new()
                .content(format!("# {}\n\n{}", giveaway.title, winners_str))
                .reference_message((giveaway.channel, giveaway.message)),
        )
        .await?;
    Ok(())
}

async fn cancel_giveaway(giveaway: &RealGiveaway, http: &impl CacheHttp) -> anyhow::Result<()> {
    let reply = match giveaway
        .channel
        .edit_message(
            http,
            giveaway.message,
            EditMessage::new()
                .content(giveaway.get_message(true))
                .components(Vec::new()),
        )
        .await
    {
        Ok(_) => true,
        Err(poise::serenity_prelude::Error::Http(
            poise::serenity_prelude::HttpError::UnsuccessfulRequest(ErrorResponse {
                error: DiscordJsonError { code: 10008, .. },
                ..
            }),
        )) => {
            //  Message not found, ignore
            false
        }
        Err(err) => Err(err)?,
    };
    if reply {
        giveaway
            .channel
            .send_message(
                http,
                CreateMessage::new()
                    .content(format!(
                        "# {}\n\nDieses Giveaway wurde abgebrochen",
                        giveaway.title
                    ))
                    .reference_message((giveaway.channel, giveaway.message)),
            )
            .await?;
    }
    Ok(())
}

#[poise::command(
    slash_command,
    default_member_permissions = "CREATE_EVENTS",
    guild_only
)]
async fn create(
    ctx: Context<'_, Arc<Database>, anyhow::Error>,
    title: String,
    description: String,
    #[min = 1] winners: Option<u32>,
    time: Option<String>,
) -> anyhow::Result<()> {
    ctx.defer().await?;
    let guild = ctx.guild_id().context("Not in a guild")?;
    let channel = ctx.channel_id();
    let winners = winners.unwrap_or(1);
    let db = ctx.data();
    let tz: Tz = {
        let db_read = db.begin_read()?;
        let table = db_read.open_table(TABLE)?;
        table
            .get(guild.get())?
            .map(|v| v.value())
            .unwrap_or_default()
            .timezone
            .parse()?
    };
    let time: Option<DateTime<Utc>> = if let Some(time) = time {
        Some(parse_time(&time, tz).map_err(|err| {
            anyhow::Error::msg(format!(
                "Fehler beim parsen der Zeit: {} --- {}",
                &time[..(time.len() - err.len())],
                err
            ))
        })?)
    } else {
        None
    };
    let id: GiveawayId = GiveawayId(rand::random());
    let content = RealGiveaway::get_message_early(&title, &description, time.as_ref(), false);
    let ar = CreateActionRow::Buttons(Vec::from([
        CreateButton::new(serde_json::to_string(&UserAction::Add(id)).unwrap())
            .label("Dabei")
            .style(poise::serenity_prelude::ButtonStyle::Success),
        CreateButton::new(serde_json::to_string(&UserAction::Remove(id)).unwrap())
            .label("Raus")
            .style(poise::serenity_prelude::ButtonStyle::Danger),
        CreateButton::new(serde_json::to_string(&UserAction::Cancel(id)).unwrap())
            .label("Abbrechen")
            .style(poise::serenity_prelude::ButtonStyle::Secondary),
        CreateButton::new(serde_json::to_string(&UserAction::Finish(id)).unwrap())
            .label("Abschließen")
            .style(poise::serenity_prelude::ButtonStyle::Secondary),
    ]));
    let message = ctx
        .send(
            CreateReply::default()
                .content(content)
                .reply(true)
                .components(vec![ar]),
        )
        .await?
        .message()
        .await?
        .id;

    let giveaway: Giveaway = RealGiveaway {
        title,
        description,
        participants: HashSet::new(),
        winners,
        channel,
        message,
        time,
    }
    .into();
    db_write(db, guild, move |state| state.giveaways.insert(id, giveaway))?;

    if let Some(time) = time {
        let http = MyHttpCache::new(
            ctx.serenity_context().http.clone(),
            ctx.serenity_context().cache.clone(),
        );
        let db = db.clone();
        tokio::spawn(async move {
            finish_task(guild, id, time, db, http).await.unwrap();
        });
    }
    Ok(())
}

async fn timezone_autocomplete<'a>(
    _ctx: poise::Context<'a, Arc<Database>, anyhow::Error>,
    part: &'a str,
) -> impl Iterator<Item = &'static str> + 'a {
    chrono_tz::TZ_VARIANTS
        .iter()
        .filter(move |tz| tz.name().starts_with(part))
        .map(|tz| tz.name())
}

#[poise::command(
    slash_command,
    default_member_permissions = "ADMINISTRATOR",
    guild_only
)]
async fn timezone(
    ctx: poise::Context<'_, Arc<Database>, anyhow::Error>,
    #[autocomplete = "timezone_autocomplete"] timezone: Tz,
) -> anyhow::Result<()> {
    ctx.defer_ephemeral().await?;
    let old = db_write(ctx.data(), ctx.guild_id().unwrap(), move |state| {
        let tz: Tz = state.timezone.parse().unwrap();
        state.timezone = timezone.to_string();
        tz
    })?;
    ctx.reply(format!("Zeitzone von {} zu {} geändert.", old, timezone))
        .await?;
    Ok(())
}

#[poise::command(slash_command, guild_only)]
async fn info(ctx: poise::Context<'_, Arc<Database>, anyhow::Error>) -> anyhow::Result<()> {
    //ctx.defer_ephemeral().await?;
    let db_read = ctx.data().begin_read()?;
    let (giveaway_count, timezone) = {
        db_read
            .open_table(TABLE)?
            .get(ctx.guild_id().unwrap().get())?
            .map(|v| v.value())
            .map(|state| (state.giveaways.len(), state.timezone.parse().unwrap()))
    }
    .unwrap_or((0, Tz::CET));
    db_read.close()?;

    let message = format!(
        r#"
Dieser Bot erstellt Giveaways und stellt rudimentäre Befehle zur Verfügung.

Befehle:
/create <Titel> <Beschreibung> [Gewinner: Anzahl Gewinner] [Zeit: Ende des Giveaways]
    Erstellt ein neues Giveaway in diesem Kanal.
    Berechtigung: CREATE_EVENTS
/timezone
    Ändern der verwendeten Zeitzone für diesen Server.
    Standart: CET bzw. CEST (Central Europian [Summer-] Time)
    Berechtigung: ADMINISTRATOR
/clear <Nutzer>
    Löscht alle Nachrichten des jeweiligen Nutzers, nützlich um Spam im Nachgang zu entfernen.
    Wenn es sehr viele Nachrichten gibt, wird das Löschen auf Grund einiger Begrenzungen von Discord lange dauern. Bitte habe etwas Geduld.
    Berechtigung: BAN_MEMBERS
/clear_all
    Leert den gesamten aktuellen Kanal.
    Wenn es sehr viele Nachrichten gibt, wird das Löschen auf Grund einiger Begrenzungen von Discord lange dauern. Bitte habe etwas Geduld.
    Berechtigung: MANAGE_CHANNELS
/info
    Zeigt diese Info an.

Bei Fragen zur Zeitangabe, wende dich bitte an @doEggi (<@518852275955957761>).

Anzahl der Giveaways auf diesem Server: {giveaway_count}
Aktuell verwendete Zeitzone: {timezone}

~doEggi was here...
"#
    )
    .trim()
    .to_string();
    ctx.send(
        CreateReply::default()
            .content(message)
            .reply(true)
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

fn dump_db(db: &Database) {
    println!("BEGIN DB DUMP");
    let db = db.begin_read().unwrap();
    let table = db.open_table(TABLE).unwrap();
    for entry in table.iter().unwrap() {
        let entry = entry.unwrap();
        let guild: GuildId = GuildId::new(entry.0.value());
        let state: GuildState = entry.1.value();
        println!("  {}: {:?}", guild, state);
    }
    println!("END DB DUMP");
}

fn db_write<T>(
    db: &Database,
    guild: GuildId,
    r#fn: impl FnOnce(&mut GuildState) -> T,
) -> anyhow::Result<T> {
    let db = db.begin_write()?;
    let res = {
        let mut table = db.open_table(TABLE)?;
        let mut state = table
            .get(guild.get())?
            .map(|v| v.value())
            .unwrap_or_default();
        let res = r#fn(&mut state);
        table.insert(guild.get(), state)?;
        res
    };
    db.commit()?;
    Ok(res)
}

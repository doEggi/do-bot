use futures::StreamExt;
use poise::{
    Context, CreateReply, command,
    serenity_prelude::{
        CacheHttp, ChannelId, CreateActionRow, CreateButton, GuildId, Permissions, UserId,
    },
};
use redb::Database;
use std::sync::Arc;
use tokio::pin;

use crate::structs::UserAction;

#[poise::command(slash_command, default_member_permissions = "BAN_MEMBERS", guild_only)]
pub async fn clear(
    ctx: Context<'_, Arc<Database>, anyhow::Error>,
    user: UserId,
) -> anyhow::Result<()> {
    let ar = CreateActionRow::Buttons(Vec::from([
        CreateButton::new(
            serde_json::to_string(&UserAction::Clear(Some((ctx.guild_id().unwrap(), user))))
                .unwrap(),
        )
        .label("Ich bin sicher")
        .style(poise::serenity_prelude::ButtonStyle::Danger),
        CreateButton::new(serde_json::to_string(&UserAction::Clear(None)).unwrap())
            .label("Abbrechen")
            .style(poise::serenity_prelude::ButtonStyle::Secondary),
    ]));
    ctx.send(
        CreateReply::default()
            .content(format!(
                "Sollen wirklich alle Nachrichten auf diesem Server des Nutzers <@{}> gelöscht werden?",
                user
            ))
            .reply(true)
            .ephemeral(true)
            .components(vec![ar]),
    )
    .await?;
    Ok(())
}

#[command(
    slash_command,
    default_member_permissions = "MANAGE_CHANNELS",
    guild_only
)]
pub async fn clear_all(ctx: Context<'_, Arc<Database>, anyhow::Error>) -> anyhow::Result<()> {
    let ar = CreateActionRow::Buttons(Vec::from([
        CreateButton::new(
            serde_json::to_string(&UserAction::ClearAll(Some(ctx.channel_id()))).unwrap(),
        )
        .label("Ich bin sicher")
        .style(poise::serenity_prelude::ButtonStyle::Danger),
        CreateButton::new(serde_json::to_string(&UserAction::ClearAll(None)).unwrap())
            .label("Abbrechen")
            .style(poise::serenity_prelude::ButtonStyle::Secondary),
    ]));
    ctx.send(
        CreateReply::default()
            .content("Soll dieser Kanal wirklich geleert werden?")
            .reply(true)
            .ephemeral(true)
            .components(vec![ar]),
    )
    .await?;
    Ok(())
}

pub async fn clear_user(
    http: &impl CacheHttp,
    guild: GuildId,
    user: UserId,
) -> anyhow::Result<usize> {
    let mut count = 0usize;
    for (channel, _) in guild.channels(http.http()).await? {
        let fut = channel.messages_iter(http.http()).filter(|mes| {
            futures::future::ready(mes.as_ref().is_ok_and(|mes| mes.author.id == user))
        });
        pin!(fut);
        while let Some(Ok(mes)) = fut.next().await {
            if mes.delete(http).await.is_ok() {
                count += 1;
            }
        }
    }
    Ok(count)
}

pub async fn clear_channel(http: &impl CacheHttp, channel: ChannelId) -> anyhow::Result<()> {
    let fut = channel.messages_iter(http.http());
    pin!(fut);
    while let Some(Ok(mes)) = fut.next().await {
        mes.delete(http).await?;
    }
    Ok(())
}

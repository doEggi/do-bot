use bincode::{Decode, Encode};
use chrono::{DateTime, Utc};
use poise::serenity_prelude::{Cache, CacheHttp, ChannelId, GuildId, Http, MessageId, UserId};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

#[derive(Debug, Clone)]
pub struct MyHttpCache(Arc<Http>, Arc<Cache>);

impl MyHttpCache {
    pub fn new(http: Arc<Http>, cache: Arc<Cache>) -> Self {
        Self(http, cache)
    }
}

impl CacheHttp for MyHttpCache {
    fn http(&self) -> &Http {
        &*self.0
    }

    fn cache(&self) -> Option<&Arc<Cache>> {
        Some(&self.1)
    }
}

#[derive(Debug, Encode, Decode)]
pub struct GuildState {
    pub timezone: String,
    pub giveaways: HashMap<GiveawayId, Giveaway>,
}

impl Default for GuildState {
    fn default() -> Self {
        Self {
            timezone: chrono_tz::CET.name().to_string(),
            giveaways: HashMap::new(),
        }
    }
}

/// This is just a data collection, no functionality behind it
#[derive(Debug, Clone, Encode, Decode)]
pub struct Giveaway {
    pub title: String,
    pub description: String,
    pub participants: HashSet<u64>,
    pub winners: u32,
    pub channel: u64,
    pub message: u64,
    pub time: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct RealGiveaway {
    pub title: String,
    pub description: String,
    pub participants: HashSet<UserId>,
    pub winners: u32,
    pub channel: ChannelId,
    pub message: MessageId,
    pub time: Option<DateTime<Utc>>,
}

impl RealGiveaway {
    pub fn get_message(&self, past: bool) -> String {
        Self::get_message_early(&self.title, &self.description, self.time.as_ref(), past)
    }

    pub fn get_message_early(
        title: &str,
        description: &str,
        time: Option<&DateTime<Utc>>,
        past: bool,
    ) -> String {
        let time_str = time
            .map(|t| {
                format!(
                    "\n\n{}: <t:{}:R>",
                    match past {
                        true => "Endete",
                        false => "Endet",
                    },
                    //  Event is finished before time ran out, so we show current time as ending
                    if past && time.is_some_and(|t| t > &Utc::now()) {
                        Utc::now().timestamp()
                    } else {
                        t.timestamp()
                    }
                )
            })
            .unwrap_or_default();
        format!("# {title}\n\n{description}{time_str}")
    }
}

impl From<Giveaway> for RealGiveaway {
    fn from(value: Giveaway) -> Self {
        RealGiveaway {
            title: value.title,
            description: value.description,
            participants: value
                .participants
                .into_iter()
                .map(|user| UserId::from(user))
                .collect(),
            winners: value.winners,
            channel: ChannelId::from(value.channel),
            message: MessageId::from(value.message),
            time: value
                .time
                .map(|ts| DateTime::from_timestamp(ts, 0).unwrap().to_utc()),
        }
    }
}

impl From<RealGiveaway> for Giveaway {
    fn from(value: RealGiveaway) -> Self {
        Giveaway {
            title: value.title,
            description: value.description,
            participants: value
                .participants
                .into_iter()
                .map(|user| user.get())
                .collect(),
            winners: value.winners,
            channel: value.channel.get(),
            message: value.message.get(),
            time: value.time.map(|time| time.timestamp()),
        }
    }
}

#[derive(
    Debug, Clone, Copy, Encode, Decode, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize,
)]
pub struct GiveawayId(pub u64);

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub enum UserAction {
    Add(GiveawayId),
    Remove(GiveawayId),
    Finish(GiveawayId),
    Cancel(GiveawayId),
    ClearAll(Option<ChannelId>),
    Clear(Option<(GuildId, UserId)>),
}

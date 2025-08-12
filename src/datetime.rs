use chrono::{DateTime, Days, NaiveDate, NaiveDateTime, NaiveTime, TimeDelta, Utc};
use chrono_tz::Tz;
use nom::{
    Parser,
    branch::{alt, permutation},
    bytes::complete::tag,
    character::complete::digit1,
    combinator::{map_res, opt},
    error::{ErrorKind, context},
};
use std::str::FromStr;

pub type IResult<I, O, E = (I, ErrorKind)> = Result<(I, O), nom::Err<E>>;

pub fn parse_time(inp: &str, tz: Tz) -> Result<DateTime<Utc>, &str> {
    alt((
        mixed(tz),
        abs(tz),
        full_rel.map_opt(|td| Utc::now().checked_add_signed(td)),
    ))
    .parse(inp)
    .map_err(|err| match err {
        nom::Err::Failure((str, _)) => str,
        nom::Err::Error((str, _)) => str,
        nom::Err::Incomplete(_) => "",
    })
    .and_then(|(rem, res)| match rem.is_empty() {
        true => Ok(res),
        false => Err(rem),
    })
}

fn mixed(tz: Tz) -> impl Fn(&str) -> IResult<&str, DateTime<Utc>> {
    move |inp| {
        alt((
            (full_part_rel, tag(" "), full_time),
            (full_time, tag(" "), full_part_rel).map(|(ft, y, t)| (t, y, ft)),
        ))
        .map_opt(|(t, _, ft)| {
            Utc::now()
                .with_timezone(&tz)
                .checked_add_signed(t)?
                .with_time(ft)
                .single()
                .map(|dt| dt.to_utc())
        })
        .parse(inp)
    }
}

fn full_part_rel(inp: &str) -> IResult<&str, TimeDelta> {
    (
        opt(tag_maybe_lowercase("In ")),
        part_rel,
        opt((alt((tag(" und "), tag(", "), tag(" "))), full_part_rel)),
    )
        .map_opt(|(_, mut time, next)| {
            if let Some((_, next)) = next {
                time = time.checked_add(&next)?;
            }
            Some(time)
        })
        .parse(inp)
}

fn part_rel(inp: &str) -> IResult<&str, TimeDelta> {
    permutation((opt(rel_days), opt(rel_weeks)))
        .map_opt(|(d, w)| {
            let mut time = TimeDelta::zero();
            for t in [d, w].into_iter().filter_map(|t| t) {
                time = time.checked_add(&t)?;
            }
            Some(time)
        })
        .map_opt(|t| (!t.is_zero()).then_some(t))
        .parse(inp)
}

#[allow(dead_code)]
fn full_rel(inp: &str) -> IResult<&str, TimeDelta> {
    (
        opt(tag_maybe_lowercase("In ")),
        rel,
        opt((alt((tag(" und "), tag(", "), tag(" "))), full_rel)),
    )
        .map_opt(|(_, mut time, next)| {
            if let Some((_, next)) = next {
                time = time.checked_add(&next)?;
            }
            Some(time)
        })
        .parse(inp)
}

fn rel(inp: &str) -> IResult<&str, TimeDelta> {
    permutation((
        opt(rel_seconds),
        opt(rel_minutes),
        opt(rel_hours),
        opt(rel_days),
        opt(rel_weeks),
    ))
    .map_opt(|(s, mi, h, d, w)| {
        let mut time = TimeDelta::zero();
        for t in [s, mi, h, d, w].into_iter().filter_map(|t| t) {
            time = time.checked_add(&t)?;
        }
        Some(time)
    })
    .map_opt(|t| (!t.is_zero()).then_some(t))
    .parse(inp)
}

fn rel_seconds(inp: &str) -> IResult<&str, TimeDelta> {
    context(
        "rel_seconds",
        (
            number::<i64>,
            alt((
                (opt(tag(" ")), alt((tag("sec"), tag("s")))).map(|_| ()),
                (
                    tag(" "),
                    alt((
                        (tag_maybe_lowercase("Sekunden")),
                        (tag_maybe_lowercase("Sekunde")),
                    )),
                )
                    .map(|_| ()),
            )),
        )
            .map_opt(|(n, _)| TimeDelta::try_seconds(n)),
    )
    .parse(inp)
}

fn rel_minutes(inp: &str) -> IResult<&str, TimeDelta> {
    context(
        "rel_minutes",
        (
            number::<i64>,
            alt((
                (opt(tag(" ")), alt((tag("min"), tag("m")))).map(|_| ()),
                (
                    tag(" "),
                    alt((
                        tag_maybe_lowercase("Minuten"),
                        tag_maybe_lowercase("Minute"),
                    )),
                )
                    .map(|_| ()),
            )),
        )
            .map_opt(|(n, _)| TimeDelta::try_minutes(n)),
    )
    .parse(inp)
}

fn rel_hours(inp: &str) -> IResult<&str, TimeDelta> {
    context(
        "rel_hours",
        (
            number::<i64>,
            alt((
                (opt(tag(" ")), tag("h")).map(|_| ()),
                (
                    tag(" "),
                    alt((
                        (tag_maybe_lowercase("Stunden")),
                        (tag_maybe_lowercase("Stunde")),
                    )),
                )
                    .map(|_| ()),
            )),
        )
            .map_opt(|(n, _)| TimeDelta::try_hours(n)),
    )
    .parse(inp)
}

fn rel_days(inp: &str) -> IResult<&str, TimeDelta> {
    context(
        "rel_days",
        (
            number::<i64>,
            alt((
                (opt(tag(" ")), tag("d")).map(|_| ()),
                (
                    tag(" "),
                    alt((
                        (tag_maybe_lowercase("Tagen")),
                        (tag_maybe_lowercase("Tage")),
                        (tag_maybe_lowercase("Tag")),
                    )),
                )
                    .map(|_| ()),
            )),
        )
            .map_opt(|(n, _)| TimeDelta::try_days(n)),
    )
    .parse(inp)
}

fn rel_weeks(inp: &str) -> IResult<&str, TimeDelta> {
    context(
        "rel_weeks",
        (
            number::<i64>,
            alt((
                (opt(tag(" ")), tag("w")).map(|_| ()),
                (
                    tag(" "),
                    alt((
                        (tag_maybe_lowercase("Wochen")),
                        (tag_maybe_lowercase("Woche")),
                    )),
                )
                    .map(|_| ()),
            )),
        )
            .map_opt(|(n, _)| TimeDelta::try_weeks(n)),
    )
    .parse(inp)
}

fn abs(tz: Tz) -> impl Fn(&str) -> IResult<&str, DateTime<Utc>> {
    move |inp| {
        context(
            "abs",
            alt((
                (full_date, tag(" "), full_time).map(|(d, _, t)| (d, t)),
                (full_time, tag(" "), full_date).map(|(t, _, d)| (d, t)),
                (special_words(tz), tag(" "), full_time).map(|(d, _, t)| (d, t)),
                (full_time, tag(" "), special_words(tz)).map(|(t, _, d)| (d, t)),
            ))
            .map_opt(|(d, t)| NaiveDateTime::new(d, t).and_local_timezone(tz).latest())
            .map_opt(|dt| (dt > Utc::now()).then_some(dt))
            .map(|dt| dt.to_utc()),
        )
        .parse(inp)
    }
}

fn special_words(tz: Tz) -> impl Fn(&str) -> IResult<&str, NaiveDate> {
    move |inp| {
        context(
            "special_words",
            alt((
                tag_maybe_lowercase("Heute").map(|_| Utc::now().with_timezone(&tz).date_naive()),
                tag_maybe_lowercase("Morgen").map_opt(|_| {
                    Utc::now()
                        .with_timezone(&tz)
                        .date_naive()
                        .checked_add_days(Days::new(1))
                }),
                tag_maybe_lowercase("Übermorgen").map_opt(|_| {
                    Utc::now()
                        .with_timezone(&tz)
                        .date_naive()
                        .checked_add_days(Days::new(2))
                }),
            )),
        )
        .parse(inp)
    }
}

fn number<T: FromStr>(inp: &str) -> IResult<&str, T> {
    map_res(digit1, |s: &str| s.parse::<T>()).parse(inp)
}

fn full_date(inp: &str) -> IResult<&str, NaiveDate> {
    context(
        "full_date",
        (opt(tag_maybe_lowercase("Am ")), date).map(|(_, d)| d),
    )
    .parse(inp)
}

fn date(inp: &str) -> IResult<&str, NaiveDate> {
    context(
        "date",
        (
            number::<u32>,
            tag("."),
            number::<u32>,
            tag("."),
            number::<i32>,
        )
            .map_opt(|(day, _, month, _, year)| NaiveDate::from_ymd_opt(year, month, day)),
    )
    .parse(inp)
}

fn full_time(inp: &str) -> IResult<&str, NaiveTime> {
    context(
        "full_time",
        (opt(tag_maybe_lowercase("Um ")), time).map(|(_, t)| t),
    )
    .parse(inp)
}

fn time(inp: &str) -> IResult<&str, NaiveTime> {
    context(
        "time",
        (
            number::<u32>,
            tag(":"),
            number::<u32>,
            opt((tag(":"), number::<u32>)),
            opt(tag_maybe_lowercase(" Uhr")),
        )
            .map_opt(|(hour, _, min, s, _)| {
                let sec = s.map(|(_, s)| s).unwrap_or_default();
                NaiveTime::from_hms_opt(hour, min, sec)
            }),
    )
    .parse(inp)
}

fn tag_maybe_lowercase(tag_: &str) -> impl Fn(&str) -> IResult<&str, &str> {
    move |inp| alt((tag(tag_), tag(tag_.to_lowercase().as_str()))).parse(inp)
}

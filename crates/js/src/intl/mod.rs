//! The native half of `intl.js`: number, plural and date formatting,
//! time zones, and the names of regions, languages and the like.
//!
//! `intl.js` keeps the JS surface (locale negotiation, reading and checking
//! options, resolvedOptions objects, the error messages) and hands the
//! resolved options here. Each set of options becomes a formatter, built
//! once and kept for the life of the page; identical options share one, so
//! a page that builds a DateTimeFormat per call does not build a pattern
//! per call. Formatters are referred to from JS by number.
//!
//! None of this exists until a page first uses Intl: `lazy.js` calls
//! `native.intlNatives()` when it loads `intl.js`. The locale and zone
//! tables are unpacked once per process, on first use.

mod date;
mod names;
mod number;
mod tzdata;
mod zones;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use rquickjs::{Array, Ctx, Exception, Function, Object, Result, Value};

use date::DateFormat;
use number::{Num, NumberFormat};
use zones::Zone;

/// The formatters a page has made.
#[derive(Default)]
struct State {
    numbers: Vec<NumberFormat>,
    number_ids: HashMap<NumberFormat, u32>,
    dates: Vec<DateFormat>,
    date_ids: HashMap<DateKey, u32>,
}

/// What a date formatter is made from.
#[derive(PartialEq, Eq, Hash)]
struct DateKey {
    locale: usize,
    zone: Zone,
    hc: String,
    date_style: Option<usize>,
    time_style: Option<usize>,
    skeleton: Option<String>,
}

/// Adds `native.intlNatives`, which makes the functions `intl.js` takes.
pub(crate) fn install<'js>(ctx: &Ctx<'js>, native: &Object<'js>) -> Result<()> {
    native.set(
        "intlNatives",
        Function::new(ctx.clone(), |ctx: Ctx<'js>| natives(&ctx))?,
    )
}

fn number<'a>(state: &'a State, ctx: &Ctx<'_>, id: u32) -> Result<&'a NumberFormat> {
    state
        .numbers
        .get(id as usize)
        .ok_or_else(|| Exception::throw_internal(ctx, "no such number format"))
}

fn date<'a>(state: &'a State, ctx: &Ctx<'_>, id: u32) -> Result<&'a DateFormat> {
    state
        .dates
        .get(id as usize)
        .ok_or_else(|| Exception::throw_internal(ctx, "no such date format"))
}

/// A time to format, as `new Date(t)` would keep it, or the RangeError
/// formatting it throws.
fn time(ctx: &Ctx<'_>, t: f64) -> Result<f64> {
    if !t.is_finite() || t.abs() > 8.64e15 {
        return Err(Exception::throw_range(ctx, "Invalid time value"));
    }
    Ok(t.trunc() + 0.0)
}

/// What the JS this replaced threw for a day period it could not look up.
fn no_day_period(ctx: &Ctx<'_>) -> impl FnOnce(date::NoDayPeriod) -> rquickjs::Error {
    |date::NoDayPeriod| Exception::throw_type(ctx, "cannot read property '1' of undefined")
}

fn date_parts(ctx: &Ctx<'_>, format: &DateFormat, t: f64) -> Result<Vec<date::Part>> {
    format.parts(time(ctx, t)?).map_err(no_day_period(ctx))
}

/// Parts as an array of `{ type, value }` objects.
fn parts_array<'js, V: rquickjs::IntoJs<'js>>(
    ctx: &Ctx<'js>,
    parts: impl IntoIterator<Item = (&'static str, V)>,
) -> Result<Array<'js>> {
    let array = Array::new(ctx.clone())?;
    for (i, (ty, value)) in parts.into_iter().enumerate() {
        let part = Object::new(ctx.clone())?;
        part.set("type", ty)?;
        part.set("value", value)?;
        array.set(i, part)?;
    }
    Ok(array)
}

fn natives<'js>(ctx: &Ctx<'js>) -> Result<Object<'js>> {
    let state = Rc::new(RefCell::new(State::default()));
    let o = Object::new(ctx.clone())?;

    let s = Rc::clone(&state);
    o.set(
        "numberFormatter",
        Function::new(
            ctx.clone(),
            move |r: Object<'js>, auto: bool| -> Result<u32> {
                let format = NumberFormat::from_options(&r, auto)?;
                let mut state = s.borrow_mut();
                if let Some(&id) = state.number_ids.get(&format) {
                    return Ok(id);
                }
                let id = state.numbers.len() as u32;
                state.numbers.push(format.clone());
                state.number_ids.insert(format, id);
                Ok(id)
            },
        )?,
    )?;
    let s = Rc::clone(&state);
    o.set(
        "numberFormat",
        Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>, id: u32, x: Value<'js>| -> Result<String> {
                let state = s.borrow();
                Ok(number(&state, &ctx, id)?.format(&Num::from_js(&x)?))
            },
        )?,
    )?;
    let s = Rc::clone(&state);
    o.set(
        "numberToParts",
        Function::new(ctx.clone(), move |ctx: Ctx<'js>, id: u32, x: Value<'js>| {
            let parts = number(&s.borrow(), &ctx, id)?.parts(&Num::from_js(&x)?);
            parts_array(&ctx, parts)
        })?,
    )?;
    let s = Rc::clone(&state);
    o.set(
        "pluralSelect",
        Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>, id: u32, n: f64, ordinal: bool| -> Result<&'static str> {
                Ok(number(&s.borrow(), &ctx, id)?.plural(n, ordinal))
            },
        )?,
    )?;

    o.set(
        "timeZoneName",
        Function::new(ctx.clone(), |tz: String| {
            zones::time_zone(&tz).map(zones::zone_name)
        })?,
    )?;
    o.set(
        "hostTimeZone",
        Function::new(ctx.clone(), || zones::host().name.clone())?,
    )?;
    o.set(
        "timeZones",
        Function::new(ctx.clone(), || {
            let mut names: Vec<&str> = zones::zone_names()
                .iter()
                .map(String::as_str)
                .filter(|name| zones::is_place(name))
                .collect();
            names.sort_unstable();
            names
        })?,
    )?;
    o.set(
        "currencies",
        Function::new(ctx.clone(), number::currencies)?,
    )?;
    o.set(
        "displayName",
        Function::new(ctx.clone(), |table: String, code: String| {
            names::name(&table, &code)
        })?,
    )?;
    o.set(
        "units",
        Function::new(ctx.clone(), || {
            number::UNITS.iter().map(|u| u.0).collect::<Vec<_>>()
        })?,
    )?;

    let s = Rc::clone(&state);
    o.set(
        "dateFormatter",
        Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>,
                  locale: u32,
                  zone: Option<String>,
                  hc: String,
                  date_style: i32,
                  time_style: i32,
                  skeleton: Option<String>|
                  -> Result<u32> {
                let zone = match zone {
                    None => Zone::Host,
                    Some(name) => zones::time_zone(&name).ok_or_else(|| {
                        Exception::throw_range(
                            &ctx,
                            &format!("Invalid time zone specified: {name}"),
                        )
                    })?,
                };
                let style = |i: i32| usize::try_from(i).ok().filter(|&i| i < 4);
                let key = DateKey {
                    locale: (locale as usize).min(date::LOCALES.len() - 1),
                    zone,
                    hc,
                    date_style: style(date_style),
                    time_style: style(time_style),
                    skeleton,
                };
                let mut state = s.borrow_mut();
                if let Some(&id) = state.date_ids.get(&key) {
                    return Ok(id);
                }
                let format = DateFormat::new(
                    key.locale,
                    key.zone,
                    &key.hc,
                    key.date_style,
                    key.time_style,
                    key.skeleton.as_deref(),
                );
                let id = state.dates.len() as u32;
                state.dates.push(format);
                state.date_ids.insert(key, id);
                Ok(id)
            },
        )?,
    )?;
    let s = Rc::clone(&state);
    o.set(
        "dateHasHour",
        Function::new(ctx.clone(), move |ctx: Ctx<'js>, id: u32| -> Result<bool> {
            Ok(date(&s.borrow(), &ctx, id)?.hour)
        })?,
    )?;
    let s = Rc::clone(&state);
    o.set(
        "dateFormat",
        Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>, id: u32, t: f64| -> Result<String> {
                let state = s.borrow();
                let t = time(&ctx, t)?;
                date(&state, &ctx, id)?
                    .format(t)
                    .map_err(no_day_period(&ctx))
            },
        )?,
    )?;
    let s = Rc::clone(&state);
    o.set(
        "dateToParts",
        Function::new(ctx.clone(), move |ctx: Ctx<'js>, id: u32, t: f64| {
            let parts = date_parts(&ctx, date(&s.borrow(), &ctx, id)?, t)?;
            parts_array(&ctx, parts)
        })?,
    )?;
    Ok(o)
}

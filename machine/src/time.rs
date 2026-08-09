//! Host-backed calendar and monotonic time (`chrono` + `std::time::Instant` registry).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::thread;
use std::time::{Duration, Instant as StdInstant};

use chrono::{DateTime, Months, NaiveDate, NaiveDateTime, TimeDelta, Utc};
use common::{BUILTIN_RESULT_VARIANTS, Value};

use crate::memory::{Heap, Member, ObjInstance, Object};

const NS_PER_SEC: i64 = 1_000_000_000;
const NS_PER_MS: i64 = 1_000_000;
const NS_PER_US: i64 = 1_000;

/// Tag indices for the virtual `time::TimeError` enum (wire order for future builtins).
#[repr(u32)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TimeErrorTag {
    InvalidInput = 0,
    Overflow = 1,
    ParseError = 2,
    Other = 3,
}

/// Monotonic instants live outside the VM heap (opaque `int` handle).
static NEXT_INSTANT_ID: AtomicU64 = AtomicU64::new(1);
static INSTANTS: LazyLock<Mutex<HashMap<u64, StdInstant>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct PeriodParts {
    years: i64,
    months: i64,
    days: i64,
    hours: i64,
    minutes: i64,
    secs: i64,
    millis: i64,
    micros: i64,
    nanos: i64,
}

// --- Result / error allocation (mirrors `io.rs`) ---

pub fn alloc_result_ok(heap: &mut Heap, payload: Value) -> Value {
    let _ = BUILTIN_RESULT_VARIANTS;
    alloc_enum(heap, 0, vec![member_from_value(heap, payload)])
}

pub fn alloc_result_err(heap: &mut Heap, payload: Value) -> Value {
    alloc_enum(heap, 1, vec![member_from_value(heap, payload)])
}

pub fn alloc_time_error(heap: &mut Heap, tag: TimeErrorTag) -> Value {
    alloc_enum(heap, tag as u32, vec![])
}

fn alloc_enum(heap: &mut Heap, tag: u32, payload: Vec<Member>) -> Value {
    heap.alloc_enum_value(tag, payload)
}

fn member_from_value(heap: &Heap, value: Value) -> Member {
    if !value.raw().is_null()
        && let Some(obj) = heap.find_object_by_addr(value.raw() as u64)
    {
        Member::Object(obj)
    } else {
        Member::Value(value)
    }
}

pub fn value_as_string(heap: &Heap, v: Value) -> Result<String, TimeErrorTag> {
    match heap.find_object_by_addr(v.raw() as u64) {
        Some(Object::String(gc)) => Ok(gc.as_ref().data.clone()),
        _ => Err(TimeErrorTag::InvalidInput),
    }
}

fn value_as_int(v: Value) -> Result<i64, TimeErrorTag> {
    Ok(v.as_int())
}

fn as_result_value(heap: &mut Heap, r: Result<Value, TimeErrorTag>) -> Value {
    match r {
        Ok(v) => alloc_result_ok(heap, v),
        Err(tag) => {
            let err = alloc_time_error(heap, tag);
            alloc_result_err(heap, err)
        }
    }
}

fn as_result_int(heap: &mut Heap, r: Result<i64, TimeErrorTag>) -> Value {
    as_result_value(heap, r.map(Value::from))
}

fn as_result_unit(heap: &mut Heap, r: Result<(), TimeErrorTag>) -> Value {
    match r {
        Ok(()) => alloc_result_ok(heap, Value::default()),
        Err(tag) => {
            let err = alloc_time_error(heap, tag);
            alloc_result_err(heap, err)
        }
    }
}

// --- Record instances (Timestamp / Period) ---

fn nanos_to_scales(nanos: i64) -> (i64, i64, i64, i64) {
    let secs = nanos.div_euclid(NS_PER_SEC);
    let millis = nanos.div_euclid(NS_PER_MS);
    let micros = nanos.div_euclid(NS_PER_US);
    (secs, millis, micros, nanos)
}

fn alloc_record(heap: &mut Heap, fields: &[(&str, i64)]) -> Value {
    let mut inst = ObjInstance::default();
    for (name, n) in fields {
        let key = heap.intern((*name).to_string());
        inst.set(key, Member::Value(Value::from(*n)));
    }
    let (obj, _) = heap.alloc(inst, Object::Instance);
    Value::from(obj.addr())
}

fn alloc_timestamp(heap: &mut Heap, nanos: i64) -> Value {
    let (secs, millis, micros, nanos) = nanos_to_scales(nanos);
    alloc_record(
        heap,
        &[
            ("secs", secs),
            ("millis", millis),
            ("micros", micros),
            ("nanos", nanos),
        ],
    )
}

fn alloc_period(heap: &mut Heap, p: PeriodParts) -> Value {
    alloc_record(
        heap,
        &[
            ("years", p.years),
            ("months", p.months),
            ("days", p.days),
            ("hours", p.hours),
            ("minutes", p.minutes),
            ("secs", p.secs),
            ("millis", p.millis),
            ("micros", p.micros),
            ("nanos", p.nanos),
        ],
    )
}

fn instance_field_i64(heap: &mut Heap, v: Value, name: &str) -> Result<i64, TimeErrorTag> {
    let Some(Object::Instance(gc)) = heap.find_object_by_addr(v.raw() as u64) else {
        return Err(TimeErrorTag::InvalidInput);
    };
    let key = heap.intern(name.to_string());
    match gc.as_ref().get(key) {
        Some(Member::Value(val)) => Ok(val.as_int()),
        Some(Member::Object(_)) => Err(TimeErrorTag::InvalidInput),
        None => Err(TimeErrorTag::InvalidInput),
    }
}

fn read_timestamp_nanos(heap: &mut Heap, ts: Value) -> Result<i64, TimeErrorTag> {
    instance_field_i64(heap, ts, "nanos")
}

fn read_period(heap: &mut Heap, p: Value) -> Result<PeriodParts, TimeErrorTag> {
    Ok(PeriodParts {
        years: instance_field_i64(heap, p, "years")?,
        months: instance_field_i64(heap, p, "months")?,
        days: instance_field_i64(heap, p, "days")?,
        hours: instance_field_i64(heap, p, "hours")?,
        minutes: instance_field_i64(heap, p, "minutes")?,
        secs: instance_field_i64(heap, p, "secs")?,
        millis: instance_field_i64(heap, p, "millis")?,
        micros: instance_field_i64(heap, p, "micros")?,
        nanos: instance_field_i64(heap, p, "nanos")?,
    })
}

fn nanos_to_utc(nanos: i64) -> Result<DateTime<Utc>, TimeErrorTag> {
    let secs = nanos.div_euclid(NS_PER_SEC);
    let sub = nanos.rem_euclid(NS_PER_SEC);
    if sub > i32::MAX as i64 {
        return Err(TimeErrorTag::Overflow);
    }
    DateTime::from_timestamp(secs, sub as u32).ok_or(TimeErrorTag::Overflow)
}

fn utc_to_nanos(dt: DateTime<Utc>) -> Result<i64, TimeErrorTag> {
    let secs = dt.timestamp();
    let nsec = i64::from(dt.timestamp_subsec_nanos());
    secs.checked_mul(NS_PER_SEC)
        .and_then(|s| s.checked_add(nsec))
        .ok_or(TimeErrorTag::Overflow)
}

fn period_to_timedelta(p: &PeriodParts) -> Result<TimeDelta, TimeErrorTag> {
    let mut delta = TimeDelta::zero();
    let add = |delta: TimeDelta,
               n: i64,
               f: fn(i64) -> Option<TimeDelta>|
     -> Result<TimeDelta, TimeErrorTag> {
        if n == 0 {
            return Ok(delta);
        }
        let piece = f(n).ok_or(TimeErrorTag::Overflow)?;
        delta.checked_add(&piece).ok_or(TimeErrorTag::Overflow)
    };
    delta = add(delta, p.days, TimeDelta::try_days)?;
    delta = add(delta, p.hours, TimeDelta::try_hours)?;
    delta = add(delta, p.minutes, TimeDelta::try_minutes)?;
    delta = add(delta, p.secs, TimeDelta::try_seconds)?;
    delta = add(delta, p.millis, TimeDelta::try_milliseconds)?;
    delta = add(delta, p.micros, |n| Some(TimeDelta::microseconds(n)))?;
    delta = add(delta, p.nanos, |n| Some(TimeDelta::nanoseconds(n)))?;
    Ok(delta)
}

fn add_months(date: NaiveDate, months: i32) -> Result<NaiveDate, TimeErrorTag> {
    if months == 0 {
        return Ok(date);
    }
    if months > 0 {
        date.checked_add_months(Months::new(months as u32))
            .ok_or(TimeErrorTag::Overflow)
    } else {
        date.checked_sub_months(Months::new((-months) as u32))
            .ok_or(TimeErrorTag::Overflow)
    }
}

fn apply_period_to_nanos(nanos: i64, p: &PeriodParts) -> Result<i64, TimeErrorTag> {
    let mut dt = nanos_to_utc(nanos)?;
    let month_delta = p
        .years
        .checked_mul(12)
        .and_then(|y| y.checked_add(p.months))
        .ok_or(TimeErrorTag::Overflow)?;
    if month_delta != 0 {
        let date = add_months(dt.date_naive(), month_delta as i32)?;
        let naive = NaiveDateTime::new(date, dt.time());
        dt = DateTime::from_naive_utc_and_offset(naive, Utc);
    }
    let delta = period_to_timedelta(p)?;
    if delta != TimeDelta::zero() {
        dt = dt.checked_add_signed(delta).ok_or(TimeErrorTag::Overflow)?;
    }
    utc_to_nanos(dt)
}

fn period_add_parts(a: PeriodParts, b: PeriodParts) -> Result<PeriodParts, TimeErrorTag> {
    let sum = |x: i64, y: i64| x.checked_add(y).ok_or(TimeErrorTag::Overflow);
    Ok(PeriodParts {
        years: sum(a.years, b.years)?,
        months: sum(a.months, b.months)?,
        days: sum(a.days, b.days)?,
        hours: sum(a.hours, b.hours)?,
        minutes: sum(a.minutes, b.minutes)?,
        secs: sum(a.secs, b.secs)?,
        millis: sum(a.millis, b.millis)?,
        micros: sum(a.micros, b.micros)?,
        nanos: sum(a.nanos, b.nanos)?,
    })
}

fn period_sub_parts(a: PeriodParts, b: PeriodParts) -> Result<PeriodParts, TimeErrorTag> {
    let sub = |x: i64, y: i64| x.checked_sub(y).ok_or(TimeErrorTag::Overflow);
    Ok(PeriodParts {
        years: sub(a.years, b.years)?,
        months: sub(a.months, b.months)?,
        days: sub(a.days, b.days)?,
        hours: sub(a.hours, b.hours)?,
        minutes: sub(a.minutes, b.minutes)?,
        secs: sub(a.secs, b.secs)?,
        millis: sub(a.millis, b.millis)?,
        micros: sub(a.micros, b.micros)?,
        nanos: sub(a.nanos, b.nanos)?,
    })
}

fn register_instant(start: StdInstant) -> u64 {
    let id = NEXT_INSTANT_ID.fetch_add(1, Ordering::Relaxed);
    INSTANTS.lock().expect("instant registry").insert(id, start);
    id
}

fn instant_from_value(v: Value) -> Result<StdInstant, TimeErrorTag> {
    let id = v.as_int();
    if id <= 0 {
        return Err(TimeErrorTag::InvalidInput);
    }
    INSTANTS
        .lock()
        .expect("instant registry")
        .get(&(id as u64))
        .copied()
        .ok_or(TimeErrorTag::InvalidInput)
}

// --- Public host entry points ---

/// Current UTC wall-clock time as a `Timestamp` record.
pub fn timestamp(heap: &mut Heap) -> Value {
    let nanos = Utc::now().timestamp_nanos_opt().unwrap_or(0);
    let ts = alloc_timestamp(heap, nanos);
    as_result_value(heap, Ok(ts))
}

/// Block the calling thread for `millis` milliseconds.
pub fn sleep_ms(heap: &mut Heap, millis: Value) -> Value {
    let ms = match value_as_int(millis) {
        Ok(n) if n >= 0 => n as u64,
        _ => return as_result_unit(heap, Err(TimeErrorTag::InvalidInput)),
    };
    thread::sleep(Duration::from_millis(ms));
    as_result_unit(heap, Ok(()))
}

/// Opaque monotonic instant handle (`int`).
pub fn instant_now(_heap: &mut Heap) -> Value {
    let id = register_instant(StdInstant::now());
    Value::from(id as i64)
}

/// Nanoseconds elapsed since `instant_now`.
pub fn elapsed_nanos(heap: &mut Heap, instant: Value) -> Value {
    let start = match instant_from_value(instant) {
        Ok(s) => s,
        Err(tag) => return as_result_int(heap, Err(tag)),
    };
    let nanos = start.elapsed().as_nanos();
    let n = i64::try_from(nanos).unwrap_or(i64::MAX);
    as_result_int(heap, Ok(n))
}

/// Milliseconds elapsed since `instant_now`.
pub fn elapsed_millis(heap: &mut Heap, instant: Value) -> Value {
    let start = match instant_from_value(instant) {
        Ok(s) => s,
        Err(tag) => return as_result_int(heap, Err(tag)),
    };
    let millis = start.elapsed().as_millis();
    let n = i64::try_from(millis).unwrap_or(i64::MAX);
    as_result_int(heap, Ok(n))
}

/// Build a `Period` record from nine `int` fields (years … nanos).
pub fn period(
    heap: &mut Heap,
    years: Value,
    months: Value,
    days: Value,
    hours: Value,
    minutes: Value,
    secs: Value,
    millis: Value,
    micros: Value,
    nanos: Value,
) -> Value {
    let read = |v: Value| value_as_int(v);
    let p = match (
        read(years),
        read(months),
        read(days),
        read(hours),
        read(minutes),
        read(secs),
        read(millis),
        read(micros),
        read(nanos),
    ) {
        (Ok(y), Ok(mo), Ok(d), Ok(h), Ok(mi), Ok(s), Ok(ms), Ok(us), Ok(ns)) => PeriodParts {
            years: y,
            months: mo,
            days: d,
            hours: h,
            minutes: mi,
            secs: s,
            millis: ms,
            micros: us,
            nanos: ns,
        },
        _ => return as_result_value(heap, Err(TimeErrorTag::InvalidInput)),
    };
    let period_val = alloc_period(heap, p);
    as_result_value(heap, Ok(period_val))
}

pub fn timestamp_add(heap: &mut Heap, ts: Value, p: Value) -> Value {
    let r = (|| {
        let nanos = read_timestamp_nanos(heap, ts)?;
        let period = read_period(heap, p)?;
        let out = apply_period_to_nanos(nanos, &period)?;
        Ok(alloc_timestamp(heap, out))
    })();
    as_result_value(heap, r)
}

pub fn timestamp_sub(heap: &mut Heap, ts: Value, p: Value) -> Value {
    let r = (|| {
        let nanos = read_timestamp_nanos(heap, ts)?;
        let period = read_period(heap, p)?;
        let neg = PeriodParts {
            years: -period.years,
            months: -period.months,
            days: -period.days,
            hours: -period.hours,
            minutes: -period.minutes,
            secs: -period.secs,
            millis: -period.millis,
            micros: -period.micros,
            nanos: -period.nanos,
        };
        let out = apply_period_to_nanos(nanos, &neg)?;
        Ok(alloc_timestamp(heap, out))
    })();
    as_result_value(heap, r)
}

pub fn period_add(heap: &mut Heap, a: Value, b: Value) -> Value {
    let r = (|| {
        let pa = read_period(heap, a)?;
        let pb = read_period(heap, b)?;
        Ok(alloc_period(heap, period_add_parts(pa, pb)?))
    })();
    as_result_value(heap, r)
}

pub fn period_sub(heap: &mut Heap, a: Value, b: Value) -> Value {
    let r = (|| {
        let pa = read_period(heap, a)?;
        let pb = read_period(heap, b)?;
        Ok(alloc_period(heap, period_sub_parts(pa, pb)?))
    })();
    as_result_value(heap, r)
}

/// UTC calendar date at midnight (from today's date).
pub fn date(heap: &mut Heap) -> Value {
    let today = Utc::now().date_naive();
    let naive = today.and_hms_opt(0, 0, 0).expect("midnight");
    let dt = DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc);
    match utc_to_nanos(dt) {
        Ok(nanos) => {
            let ts = alloc_timestamp(heap, nanos);
            as_result_value(heap, Ok(ts))
        }
        Err(tag) => as_result_value(heap, Err(tag)),
    }
}

/// Calendar date at midnight from `Period` year/month/day fields (time fields ignored).
pub fn date_from_period(heap: &mut Heap, p: Value) -> Value {
    let r = (|| {
        let period = read_period(heap, p)?;
        let y = i32::try_from(period.years).map_err(|_| TimeErrorTag::Overflow)?;
        let m = u32::try_from(period.months).map_err(|_| TimeErrorTag::Overflow)?;
        let d = u32::try_from(period.days).map_err(|_| TimeErrorTag::Overflow)?;
        if m == 0 || d == 0 {
            return Err(TimeErrorTag::InvalidInput);
        }
        let date = NaiveDate::from_ymd_opt(y, m, d).ok_or(TimeErrorTag::InvalidInput)?;
        let naive = date
            .and_hms_opt(0, 0, 0)
            .ok_or(TimeErrorTag::InvalidInput)?;
        let dt = DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc);
        utc_to_nanos(dt).map(|n| alloc_timestamp(heap, n))
    })();
    as_result_value(heap, r)
}

/// Unix epoch plus a `Period` offset.
pub fn date_from_epoch_period(heap: &mut Heap, p: Value) -> Value {
    let r = (|| {
        let period = read_period(heap, p)?;
        let out = apply_period_to_nanos(0, &period)?;
        Ok(alloc_timestamp(heap, out))
    })();
    as_result_value(heap, r)
}

pub fn epoch(heap: &mut Heap) -> Value {
    let ts = alloc_timestamp(heap, 0);
    as_result_value(heap, Ok(ts))
}

/// Format a `Timestamp` with a chrono format string (e.g. `"%Y-%m-%d %H:%M:%S"`).
pub fn format(heap: &mut Heap, ts: Value, fmt: Value) -> Value {
    let r = (|| {
        let nanos = read_timestamp_nanos(heap, ts)?;
        let fmt_str = value_as_string(heap, fmt)?;
        let dt = nanos_to_utc(nanos)?;
        let s = dt.format(&fmt_str).to_string();
        let gc = heap.intern(s);
        Ok(Value::from(gc.as_ptr() as *mut u8 as u64))
    })();
    as_result_value(heap, r)
}

/// Parse a UTC timestamp from `text` using a chrono format string.
pub fn parse(heap: &mut Heap, text: Value, fmt: Value) -> Value {
    let r = (|| {
        let input = value_as_string(heap, text)?;
        let fmt_str = value_as_string(heap, fmt)?;
        let naive = NaiveDateTime::parse_from_str(&input, &fmt_str)
            .map_err(|_| TimeErrorTag::ParseError)?;
        let dt = DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc);
        let nanos = utc_to_nanos(dt)?;
        Ok(alloc_timestamp(heap, nanos))
    })();
    as_result_value(heap, r)
}

fn time_wrong_arity(heap: &mut Heap) -> Value {
    as_result_value(heap, Err(TimeErrorTag::InvalidInput))
}

pub fn host_time_timestamp(heap: &mut Heap, args: &[Value]) -> Value {
    if args.is_empty() {
        timestamp(heap)
    } else {
        time_wrong_arity(heap)
    }
}

pub fn host_time_sleep_ms(heap: &mut Heap, args: &[Value]) -> Value {
    match args {
        [ms] => sleep_ms(heap, *ms),
        _ => time_wrong_arity(heap),
    }
}

pub fn host_time_instant_now(heap: &mut Heap, args: &[Value]) -> Value {
    if args.is_empty() {
        instant_now(heap)
    } else {
        time_wrong_arity(heap)
    }
}

pub fn host_time_elapsed_nanos(heap: &mut Heap, args: &[Value]) -> Value {
    match args {
        [inst] => elapsed_nanos(heap, *inst),
        _ => time_wrong_arity(heap),
    }
}

pub fn host_time_elapsed_millis(heap: &mut Heap, args: &[Value]) -> Value {
    match args {
        [inst] => elapsed_millis(heap, *inst),
        _ => time_wrong_arity(heap),
    }
}

pub fn host_time_period(heap: &mut Heap, args: &[Value]) -> Value {
    match args {
        [y, mo, d, h, mi, s, ms, us, ns] => period(heap, *y, *mo, *d, *h, *mi, *s, *ms, *us, *ns),
        _ => time_wrong_arity(heap),
    }
}

pub fn host_time_add(heap: &mut Heap, args: &[Value]) -> Value {
    match args {
        [ts, p] => timestamp_add(heap, *ts, *p),
        _ => time_wrong_arity(heap),
    }
}

pub fn host_time_sub(heap: &mut Heap, args: &[Value]) -> Value {
    match args {
        [ts, p] => timestamp_sub(heap, *ts, *p),
        _ => time_wrong_arity(heap),
    }
}

pub fn host_time_period_add(heap: &mut Heap, args: &[Value]) -> Value {
    match args {
        [a, b] => period_add(heap, *a, *b),
        _ => time_wrong_arity(heap),
    }
}

pub fn host_time_period_sub(heap: &mut Heap, args: &[Value]) -> Value {
    match args {
        [a, b] => period_sub(heap, *a, *b),
        _ => time_wrong_arity(heap),
    }
}

pub fn host_time_date(heap: &mut Heap, args: &[Value]) -> Value {
    if args.is_empty() {
        date(heap)
    } else {
        time_wrong_arity(heap)
    }
}

pub fn host_time_date_from_period(heap: &mut Heap, args: &[Value]) -> Value {
    match args {
        [p] => date_from_period(heap, *p),
        _ => time_wrong_arity(heap),
    }
}

pub fn host_time_date_from_epoch_period(heap: &mut Heap, args: &[Value]) -> Value {
    match args {
        [p] => date_from_epoch_period(heap, *p),
        _ => time_wrong_arity(heap),
    }
}

pub fn host_time_epoch(heap: &mut Heap, args: &[Value]) -> Value {
    if args.is_empty() {
        epoch(heap)
    } else {
        time_wrong_arity(heap)
    }
}

pub fn host_time_format(heap: &mut Heap, args: &[Value]) -> Value {
    match args {
        [ts, fmt] => format(heap, *ts, *fmt),
        _ => time_wrong_arity(heap),
    }
}

pub fn host_time_parse(heap: &mut Heap, args: &[Value]) -> Value {
    match args {
        [text, fmt] => parse(heap, *text, *fmt),
        _ => time_wrong_arity(heap),
    }
}

/// Pipeline wiring: `(registry_name, arity, host_fn)`.
pub const TIME_WIRING: &[(&str, usize, fn(&mut Heap, &[Value]) -> Value)] = &[
    ("time_timestamp", 0, host_time_timestamp),
    ("time_sleep_ms", 1, host_time_sleep_ms),
    ("time_instant_now", 0, host_time_instant_now),
    ("time_elapsed_nanos", 1, host_time_elapsed_nanos),
    ("time_elapsed_millis", 1, host_time_elapsed_millis),
    ("time_period", 9, host_time_period),
    ("time_add", 2, host_time_add),
    ("time_sub", 2, host_time_sub),
    ("time_period_add", 2, host_time_period_add),
    ("time_period_sub", 2, host_time_period_sub),
    ("time_date", 0, host_time_date),
    ("time_date_from_period", 1, host_time_date_from_period),
    (
        "time_date_from_epoch_period",
        1,
        host_time_date_from_epoch_period,
    ),
    ("time_epoch", 0, host_time_epoch),
    ("time_format", 2, host_time_format),
    ("time_parse", 2, host_time_parse),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn unwrap_result_ok(heap: &Heap, v: Value) -> Value {
        let Some(Object::Enum(gc)) = heap.find_object_by_addr(v.raw() as u64) else {
            panic!("expected Result enum");
        };
        assert_eq!(gc.as_ref().tag, 0, "expected Ok");
        match &gc.as_ref().payload[0] {
            Member::Value(val) => *val,
            Member::Object(o) => Value::from(o.addr()),
        }
    }

    #[test]
    fn format_parse_roundtrip() {
        let mut heap = Heap::default();
        let nanos = 1_704_067_200_i64 * NS_PER_SEC; // 2024-01-01 00:00:00 UTC
        let ts = alloc_timestamp(&mut heap, nanos);
        let fmt_v =
            Value::from(heap.intern("%Y-%m-%d %H:%M:%S".to_string()).as_ptr() as *mut u8 as u64);
        let formatted_r = format(&mut heap, ts, fmt_v);
        let formatted = unwrap_result_ok(&heap, formatted_r);
        let s = value_as_string(&heap, formatted).unwrap();
        assert_eq!(s, "2024-01-01 00:00:00");
        let round_r = parse(&mut heap, formatted, fmt_v);
        let round = unwrap_result_ok(&heap, round_r);
        assert_eq!(read_timestamp_nanos(&mut heap, round).unwrap(), nanos);
    }

    #[test]
    fn period_add_fields() {
        let mut heap = Heap::default();
        let a = alloc_period(
            &mut heap,
            PeriodParts {
                days: 1,
                hours: 2,
                ..PeriodParts::default()
            },
        );
        let b = alloc_period(
            &mut heap,
            PeriodParts {
                days: 3,
                minutes: 5,
                ..PeriodParts::default()
            },
        );
        let pv_r = period_add(&mut heap, a, b);
        let pv = unwrap_result_ok(&heap, pv_r);
        assert_eq!(instance_field_i64(&mut heap, pv, "days").unwrap(), 4);
        assert_eq!(instance_field_i64(&mut heap, pv, "hours").unwrap(), 2);
        assert_eq!(instance_field_i64(&mut heap, pv, "minutes").unwrap(), 5);
    }

    fn result_err_tag(heap: &Heap, v: Value) -> u32 {
        let Some(Object::Enum(gc)) = heap.find_object_by_addr(v.raw() as u64) else {
            panic!("expected Result enum");
        };
        assert_eq!(gc.as_ref().tag, 1, "expected Err");
        match &gc.as_ref().payload[0] {
            Member::Object(Object::Enum(err)) => err.as_ref().tag,
            _ => panic!("expected TimeError enum payload"),
        }
    }

    #[test]
    fn date_from_period_rejects_zero_month_and_instant_invalid() {
        let mut heap = Heap::default();
        let bad = alloc_period(
            &mut heap,
            PeriodParts {
                years: 2024,
                months: 0,
                days: 1,
                ..PeriodParts::default()
            },
        );
        let r = date_from_period(&mut heap, bad);
        assert_eq!(result_err_tag(&heap, r), TimeErrorTag::InvalidInput as u32);

        let elapsed = elapsed_nanos(&mut heap, Value::from(0_i64));
        assert_eq!(
            result_err_tag(&heap, elapsed),
            TimeErrorTag::InvalidInput as u32
        );
    }

    #[test]
    fn timestamp_add_one_month_from_known_epoch() {
        let mut heap = Heap::default();
        // 2024-01-01 00:00:00 UTC
        let nanos = 1_704_067_200_i64 * NS_PER_SEC;
        let ts = alloc_timestamp(&mut heap, nanos);
        let month = alloc_period(
            &mut heap,
            PeriodParts {
                months: 1,
                ..PeriodParts::default()
            },
        );
        let out_r = timestamp_add(&mut heap, ts, month);
        let out = unwrap_result_ok(&heap, out_r);
        let out_nanos = read_timestamp_nanos(&mut heap, out).unwrap();
        // 2024-02-01 00:00:00 UTC
        assert_eq!(out_nanos, 1_706_745_600_i64 * NS_PER_SEC);
    }

    #[test]
    fn time_wiring_arities() {
        assert_eq!(TIME_WIRING.len(), 16);
        let by_name: std::collections::BTreeMap<&str, usize> =
            TIME_WIRING.iter().map(|&(n, a, _)| (n, a)).collect();
        assert_eq!(by_name["time_timestamp"], 0);
        assert_eq!(by_name["time_period"], 9);
        assert_eq!(by_name["time_add"], 2);
        assert_eq!(by_name["time_sleep_ms"], 1);
    }
}

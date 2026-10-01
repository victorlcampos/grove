//! The five-field cron expressions Claude Desktop schedules its routines with, in local time:
//! when one runs next, when it was last due, and how to say it in words.

use chrono::{DateTime, Datelike, Days, Local, NaiveDate, TimeZone, Timelike};

use crate::i18n::Lang;

/// How far a search for the next or the previous run goes: far enough for a February 29th.
const SEARCH_DAYS: u64 = 366 * 5;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cron {
    minutes: Vec<u32>,
    hours: Vec<u32>,
    days: Vec<u32>,
    months: Vec<u32>,
    /// 0 is Sunday; a 7 in the expression is taken as 0.
    weekdays: Vec<u32>,
    /// Whether the day of the month or of the week was given: when both are, a day that
    /// matches either one runs, as in every cron.
    any_day: bool,
    any_weekday: bool,
}

impl Cron {
    pub fn parse(expression: &str) -> Option<Self> {
        let fields: Vec<&str> = expression.split_whitespace().collect();
        let [minute, hour, day, month, weekday] = fields[..] else {
            return None;
        };
        let mut weekdays = field(weekday, 0, 7)?;
        for w in &mut weekdays {
            *w %= 7;
        }
        weekdays.sort_unstable();
        weekdays.dedup();
        Some(Self {
            minutes: field(minute, 0, 59)?,
            hours: field(hour, 0, 23)?,
            days: field(day, 1, 31)?,
            months: field(month, 1, 12)?,
            weekdays,
            any_day: day == "*",
            any_weekday: weekday == "*",
        })
    }

    fn runs_on(&self, date: NaiveDate) -> bool {
        if !self.months.contains(&date.month()) {
            return false;
        }
        let by_day = self.days.contains(&date.day());
        let by_weekday = self
            .weekdays
            .contains(&date.weekday().num_days_from_sunday());
        match (self.any_day, self.any_weekday) {
            (true, true) => true,
            (true, false) => by_weekday,
            (false, true) => by_day,
            (false, false) => by_day || by_weekday,
        }
    }

    /// The times it runs on `date`, earliest first; a time the clock skips (a daylight saving
    /// change) is left out.
    fn times_on(&self, date: NaiveDate) -> impl DoubleEndedIterator<Item = DateTime<Local>> + '_ {
        self.hours
            .iter()
            .flat_map(move |&hour| self.minutes.iter().map(move |&minute| (hour, minute)))
            .filter_map(move |(hour, minute)| {
                let naive = date.and_hms_opt(hour, minute, 0)?;
                Local.from_local_datetime(&naive).earliest()
            })
    }

    /// The first run after `now`.
    pub fn next_after(&self, now: DateTime<Local>) -> Option<DateTime<Local>> {
        let today = now.date_naive();
        (0..SEARCH_DAYS)
            .filter_map(|n| today.checked_add_days(Days::new(n)))
            .filter(|&date| self.runs_on(date))
            .find_map(|date| self.times_on(date).find(|&time| time > now))
    }

    /// The last run due at or before `now`.
    pub fn last_before(&self, now: DateTime<Local>) -> Option<DateTime<Local>> {
        let today = now.date_naive();
        (0..SEARCH_DAYS)
            .filter_map(|n| today.checked_sub_days(Days::new(n)))
            .filter(|&date| self.runs_on(date))
            .find_map(|date| self.times_on(date).rev().find(|&time| time <= now))
    }

    /// Words for the common schedules, "weekdays 17:30" or "every day 12:07", and `None`
    /// for the others, which are shown as written.
    pub fn describe(&self, lang: Lang) -> Option<String> {
        let ([hour], [minute]) = (&self.hours[..], &self.minutes[..]) else {
            return None;
        };
        if !self.any_day || self.months.len() != 12 {
            return None;
        }
        let time = format!("{hour:02}:{minute:02}");
        let names = weekday_names(lang);
        let days = if self.any_weekday {
            every_day(lang).to_string()
        } else if self.weekdays == [1, 2, 3, 4, 5] {
            weekdays_word(lang).to_string()
        } else {
            self.weekdays
                .iter()
                .map(|&w| names[w as usize])
                .collect::<Vec<_>>()
                .join(", ")
        };
        Some(format!("{days} {time}"))
    }
}

/// One field: `*`, numbers, ranges `a-b` and steps `*/n` or `a-b/n`, separated by commas.
fn field(text: &str, min: u32, max: u32) -> Option<Vec<u32>> {
    let mut values = Vec::new();
    for part in text.split(',') {
        let (range, step) = match part.split_once('/') {
            Some((range, step)) => (range, step.parse::<u32>().ok().filter(|&s| s > 0)?),
            None => (part, 1),
        };
        let (from, to) = if range == "*" {
            (min, max)
        } else if let Some((a, b)) = range.split_once('-') {
            (a.parse().ok()?, b.parse().ok()?)
        } else {
            let value: u32 = range.parse().ok()?;
            // "5/10" runs from 5 to the end, every 10.
            (value, if part.contains('/') { max } else { value })
        };
        if from < min || to > max || from > to {
            return None;
        }
        values.extend((from..=to).step_by(step as usize));
    }
    values.sort_unstable();
    values.dedup();
    Some(values)
}

fn weekday_names(lang: Lang) -> [&'static str; 7] {
    match lang {
        Lang::En => ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"],
        Lang::Pt => ["dom", "seg", "ter", "qua", "qui", "sex", "sáb"],
    }
}

fn every_day(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "every day",
        Lang::Pt => "todo dia",
    }
}

fn weekdays_word(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "weekdays",
        Lang::Pt => "dias úteis",
    }
}

/// When a run is, near enough to read at a glance: "17:30" today, "Thu 09:05" this week,
/// "Oct 14" further away.
pub fn when(time: DateTime<Local>, now: DateTime<Local>, lang: Lang) -> String {
    let clock = format!("{:02}:{:02}", time.hour(), time.minute());
    let days = (time.date_naive() - now.date_naive()).num_days();
    if days == 0 {
        return clock;
    }
    if (-6..=6).contains(&days) {
        let name = weekday_names(lang)[time.weekday().num_days_from_sunday() as usize];
        return format!("{name} {clock}");
    }
    let day = time.day();
    match lang {
        Lang::En => format!("{} {day}", time.format("%b")),
        Lang::Pt => format!("{day:02}/{:02}", time.month()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Local> {
        Local
            .with_ymd_and_hms(y, m, d, h, min, 0)
            .earliest()
            .unwrap()
    }

    #[test]
    fn reads_lists_ranges_and_steps() {
        let cron = Cron::parse("*/15 9-11,17 * * 1-5").unwrap();
        assert_eq!(cron.minutes, [0, 15, 30, 45]);
        assert_eq!(cron.hours, [9, 10, 11, 17]);
        assert_eq!(cron.weekdays, [1, 2, 3, 4, 5]);
        assert_eq!(Cron::parse("0 0 * * 7").unwrap().weekdays, [0]);
        assert_eq!(Cron::parse("5/20 * * * *").unwrap().minutes, [5, 25, 45]);
        for bad in [
            "",
            "* * * *",
            "60 * * * *",
            "* * 0 * *",
            "*/0 * * * *",
            "a * * * *",
        ] {
            assert_eq!(Cron::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn finds_the_next_run_and_the_last_one_due() {
        // Wednesday, September 30th 2026.
        let now = at(2026, 9, 30, 17, 40);
        let weekdays = Cron::parse("30 17 * * 1-5").unwrap();
        assert_eq!(weekdays.next_after(now), Some(at(2026, 10, 1, 17, 30)));
        assert_eq!(weekdays.last_before(now), Some(at(2026, 9, 30, 17, 30)));
        // Friday evening: the next one is on Monday.
        let friday = at(2026, 10, 2, 18, 0);
        assert_eq!(weekdays.next_after(friday), Some(at(2026, 10, 5, 17, 30)));
        let wednesdays = Cron::parse("20 9 * * 3").unwrap();
        assert_eq!(wednesdays.next_after(now), Some(at(2026, 10, 7, 9, 20)));
        assert_eq!(wednesdays.last_before(now), Some(at(2026, 9, 30, 9, 20)));
        // Both days given: either one runs.
        let either = Cron::parse("0 8 1 * 1").unwrap();
        assert_eq!(either.next_after(now), Some(at(2026, 10, 1, 8, 0)));
        assert_eq!(
            either.next_after(at(2026, 10, 1, 9, 0)),
            Some(at(2026, 10, 5, 8, 0))
        );
        let leap = Cron::parse("0 0 29 2 *").unwrap();
        assert_eq!(leap.next_after(now), Some(at(2028, 2, 29, 0, 0)));
    }

    #[test]
    fn says_common_schedules_in_words() {
        let describe = |text: &str, lang| Cron::parse(text).unwrap().describe(lang);
        assert_eq!(
            describe("30 17 * * 1-5", Lang::Pt).as_deref(),
            Some("dias úteis 17:30")
        );
        assert_eq!(
            describe("7 12 * * *", Lang::En).as_deref(),
            Some("every day 12:07")
        );
        assert_eq!(
            describe("20 9 * * 3,4", Lang::Pt).as_deref(),
            Some("qua, qui 09:20")
        );
        assert_eq!(describe("*/15 * * * *", Lang::En), None);
        assert_eq!(describe("0 9 1 * *", Lang::En), None);
    }

    #[test]
    fn tells_when_a_run_is() {
        let now = at(2026, 9, 30, 10, 0);
        assert_eq!(when(at(2026, 9, 30, 17, 30), now, Lang::En), "17:30");
        assert_eq!(when(at(2026, 10, 1, 9, 5), now, Lang::Pt), "qui 09:05");
        assert_eq!(when(at(2026, 10, 14, 9, 5), now, Lang::Pt), "14/10");
        assert_eq!(when(at(2026, 10, 14, 9, 5), now, Lang::En), "Oct 14");
    }
}

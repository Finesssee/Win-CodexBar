use chrono::{DateTime, Duration, Local, NaiveDate, NaiveDateTime, TimeZone, Utc};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CursorLocalSpendSummary {
    pub total_cost_usd: f64,
    pub total_tokens: u64,
    pub row_count: usize,
}

#[derive(Debug)]
struct Row {
    at: DateTime<Utc>,
    input: u64,
    read: u64,
    write: u64,
    output: u64,
    total: Option<u64>,
    cost: f64,
}

pub fn summarize(days: u32) -> CursorLocalSpendSummary {
    summarize_paths(&paths(None), Utc::now(), days)
}

fn paths(home: Option<&Path>) -> Vec<PathBuf> {
    let base = if let Some(h) = home {
        h.join(".config").join("tokscale").join("cursor-cache")
    } else if let Ok(r) = std::env::var("TOKSCALE_CONFIG_DIR") {
        PathBuf::from(r).join("cursor-cache")
    } else {
        let Some(h) = dirs::home_dir() else {
            return vec![];
        };
        h.join(".config").join("tokscale").join("cursor-cache")
    };
    let Ok(rd) = fs::read_dir(base) else {
        return vec![];
    };
    let mut v: Vec<_> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                n.starts_with("usage") && n.ends_with(".csv") && !n.starts_with("usage.backup")
            })
        })
        .collect();
    v.sort();
    v
}

fn summarize_paths(paths: &[PathBuf], now: DateTime<Utc>, days: u32) -> CursorLocalSpendSummary {
    let cutoff =
        (now - Duration::days(i64::from(days.clamp(1, 365).saturating_sub(1)))).date_naive();
    let mut out = CursorLocalSpendSummary::default();
    for p in paths {
        for r in parse_file(p) {
            if r.at > now || r.at.date_naive() < cutoff {
                continue;
            }
            let t = r.total.unwrap_or(
                r.input
                    .saturating_add(r.read)
                    .saturating_add(r.write)
                    .saturating_add(r.output),
            );
            out.total_tokens += t;
            out.total_cost_usd += r.cost;
            out.row_count += 1;
        }
    }
    out
}

#[derive(Debug, Clone, Copy)]
struct CsvSchema {
    date: usize,
    model: usize,
    input_with_cache: usize,
    input_without_cache: usize,
    cache_read: usize,
    output: usize,
    total_tokens: Option<usize>,
    cost: usize,
}

impl CsvSchema {
    fn from_header(header: &[String]) -> Option<Self> {
        let index = |name: &str| {
            header
                .iter()
                .position(|column| normalize_header(column) == name)
        };
        Some(Self {
            date: index("date")?,
            model: index("model")?,
            input_with_cache: index("inputwithcache")?,
            input_without_cache: index("inputwithoutcache")?,
            cache_read: index("cacheread")?,
            output: index("output")?,
            total_tokens: index("totaltokens"),
            cost: index("cost")?,
        })
    }

    fn max_required_index(self) -> usize {
        [
            self.date,
            self.model,
            self.input_with_cache,
            self.input_without_cache,
            self.cache_read,
            self.output,
            self.cost,
        ]
        .into_iter()
        .max()
        .unwrap_or(0)
    }
}

fn parse_file(path: &Path) -> Vec<Row> {
    let Ok(text) = fs::read_to_string(path) else {
        return vec![];
    };
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());
    let Some(header_line) = lines.next() else {
        return vec![];
    };
    let Some(header) = parse_csv_line(header_line) else {
        return vec![];
    };
    let Some(schema) = CsvSchema::from_header(&header) else {
        return vec![];
    };
    lines.filter_map(|line| parse_row(line, schema)).collect()
}

fn parse_row(line: &str, schema: CsvSchema) -> Option<Row> {
    let columns = parse_csv_line(line)?;
    if columns.len() <= schema.max_required_index() || columns.get(schema.model)?.trim().is_empty()
    {
        return None;
    }
    let input_with_cache = parse_u64(columns.get(schema.input_with_cache)?)?;
    let input = parse_u64(columns.get(schema.input_without_cache)?)?;
    let read = parse_u64(columns.get(schema.cache_read)?)?;
    let output = parse_u64(columns.get(schema.output)?)?;
    let write = input_with_cache.saturating_sub(input);
    if input == 0 && read == 0 && write == 0 && output == 0 {
        return None;
    }
    let total = match schema.total_tokens {
        Some(index) => parse_optional_u64(columns.get(index)?)?,
        None => None,
    };
    Some(Row {
        at: parse_date(columns.get(schema.date)?)?,
        input,
        read,
        write,
        output,
        total,
        cost: parse_cost(columns.get(schema.cost)?)?,
    })
}

fn normalize_header(raw: &str) -> String {
    raw.trim()
        .chars()
        .filter(|ch| !matches!(ch, ' ' | '_' | '-'))
        .flat_map(char::to_lowercase)
        .collect()
}

fn parse_csv_line(line: &str) -> Option<Vec<String>> {
    let mut columns = Vec::new();
    let mut current = String::new();
    let mut chars = line.chars().peekable();
    let mut quoted = false;
    let mut closed_quote = false;
    while let Some(ch) = chars.next() {
        match ch {
            '"' if quoted && chars.peek() == Some(&'"') => {
                chars.next();
                current.push('"');
            }
            '"' if quoted => {
                quoted = false;
                closed_quote = true;
            }
            '"' if current.trim().is_empty() && !closed_quote => quoted = true,
            ',' if !quoted => {
                columns.push(current.trim().to_string());
                current.clear();
                closed_quote = false;
            }
            ch if closed_quote && !ch.is_whitespace() => return None,
            ch => current.push(ch),
        }
    }
    if quoted {
        return None;
    }
    columns.push(current.trim().to_string());
    Some(columns)
}

fn parse_u64(raw: &str) -> Option<u64> {
    let normalized = raw.trim().replace(',', "");
    (!normalized.is_empty())
        .then(|| normalized.parse::<u64>().ok())
        .flatten()
}

fn parse_optional_u64(raw: &str) -> Option<Option<u64>> {
    let normalized = raw.trim();
    if normalized.is_empty() {
        return Some(None);
    }
    parse_u64(normalized).map(Some)
}

fn parse_cost(raw: &str) -> Option<f64> {
    let normalized = raw.trim();
    if normalized.is_empty()
        || normalized == "-"
        || normalized.eq_ignore_ascii_case("included")
        || normalized.eq_ignore_ascii_case("nan")
    {
        return Some(0.0);
    }
    normalized
        .replace(['$', ','], "")
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && *value >= 0.0)
}

fn parse_date(raw: &str) -> Option<DateTime<Utc>> {
    let raw = raw.trim();
    if let Ok(value) = DateTime::parse_from_rfc3339(raw) {
        return Some(value.with_timezone(&Utc));
    }
    for format in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S"] {
        if let Ok(value) = NaiveDateTime::parse_from_str(raw, format) {
            return Some(value.and_utc());
        }
    }
    let noon = NaiveDate::parse_from_str(raw, "%Y-%m-%d")
        .ok()?
        .and_hms_opt(12, 0, 0)?;
    Some(
        Local
            .from_local_datetime(&noon)
            .single()
            .map(|value| value.with_timezone(&Utc))
            .unwrap_or_else(|| noon.and_utc()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn v3_total() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("usage-v3.csv");
        fs::write(&p,"Date,Kind,Provider,Session,Model,Requests,Input With Cache,Input Without Cache,Cache Read,Output,Total Tokens,Cost
2026-08-24T10:00:00Z,usage,cursor,s1,test-model,1,150,100,20,30,999,1.25
").unwrap();
        let r = parse_file(&p);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].write, 50);
        assert_eq!(r[0].total, Some(999));
    }
    #[test]
    fn filters_old() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("usage.csv");
        fs::write(
            &p,
            "Date,Model,Input With Cache,Input Without Cache,Cache Read,Output,Total Tokens,Cost
2026-08-23,test-model,10,8,1,2,20,0.50
2026-08-01,test-model,10,8,1,2,20,9.00
",
        )
        .unwrap();
        let now = DateTime::parse_from_rfc3339("2026-08-24T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let x = summarize_paths(&[p], now, 7);
        assert_eq!(x.row_count, 1);
        assert_eq!(x.total_tokens, 20);
        assert!((x.total_cost_usd - 0.5).abs() < 1e-9);
    }

    #[test]
    fn header_names_drive_schema_and_quoted_fields_are_decoded() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("usage.csv");
        fs::write(
            &p,
            r#"Cost,Output,Model,Date,Cache Read,Input Without Cache,Input With Cache,Total Tokens
1.50,2,"model, ""quoted""",2026-08-24T10:00:00Z,1,8,10,20
"#,
        )
        .unwrap();
        let rows = parse_file(&p);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].total, Some(20));
        assert!((rows[0].cost - 1.5).abs() < 1e-9);
    }

    #[test]
    fn malformed_required_numeric_field_rejects_row_instead_of_becoming_zero() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("usage.csv");
        fs::write(
            &p,
            "Date,Model,Input With Cache,Input Without Cache,Cache Read,Output,Total Tokens,Cost
2026-08-24,test-model,not-a-number,8,1,2,20,0.50
",
        )
        .unwrap();
        assert!(parse_file(&p).is_empty());
    }
}

use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, Utc};
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
            out.total_tokens = out.total_tokens + t;
            out.total_cost_usd += r.cost;
            out.row_count += 1;
        }
    }
    out
}

fn parse_file(path: &Path) -> Vec<Row> {
    let Ok(text) = fs::read_to_string(path) else {
        return vec![];
    };
    let mut ls = text.lines().filter(|l| !l.trim().is_empty());
    let Some(h) = ls.next() else { return vec![] };
    let hdr = csv(h);
    let kind = hdr.iter().any(|c| c.eq_ignore_ascii_case("kind"));
    let (model, iw, io, read, out, cost) = if kind && hdr.len() >= 12 {
        (4, 6, 7, 8, 9, 11)
    } else if kind {
        (2, 4, 5, 6, 7, 9)
    } else {
        (1, 2, 3, 4, 5, 7)
    };
    let ti = cost - 1;
    let authoritative = hdr
        .get(ti)
        .is_some_and(|c| c.to_ascii_lowercase().contains("total"));
    ls.filter_map(|l| {
        let c = csv(l);
        if c.len() <= cost || c.get(model)?.trim().is_empty() {
            return None;
        }
        let iw = n(c.get(iw)?);
        let input = n(c.get(io)?);
        let read = n(c.get(read)?);
        let output = n(c.get(out)?);
        let write = iw.saturating_sub(input);
        if input == 0 && read == 0 && write == 0 && output == 0 {
            return None;
        }
        Some(Row {
            at: date(c.first()?)?,
            input,
            read,
            write,
            output,
            total: authoritative.then(|| n(c.get(ti).map(String::as_str).unwrap_or("0"))),
            cost: money(c.get(cost)?),
        })
    })
    .collect()
}

fn csv(line: &str) -> Vec<String> {
    let mut v = vec![];
    let mut s = String::new();
    let mut q = false;
    for ch in line.chars() {
        match ch {
            '"' => q = !q,
            ',' if !q => {
                v.push(s.trim().to_string());
                s.clear()
            }
            _ => s.push(ch),
        }
    }
    v.push(s.trim().to_string());
    v
}
fn n(s: &str) -> u64 {
    s.trim().replace(',', "").parse().unwrap_or(0)
}
fn money(s: &str) -> f64 {
    let s = s.trim();
    if s.is_empty()
        || s == "-"
        || s.eq_ignore_ascii_case("included")
        || s.eq_ignore_ascii_case("nan")
    {
        0.0
    } else {
        s.replace('$', "").replace(',', "").parse().unwrap_or(0.0)
    }
}
fn date(s: &str) -> Option<DateTime<Utc>> {
    let s = s.trim();
    if let Ok(v) = DateTime::parse_from_rfc3339(s) {
        return Some(v.with_timezone(&Utc));
    }
    for f in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S"] {
        if let Ok(v) = NaiveDateTime::parse_from_str(s, f) {
            return Some(v.and_utc());
        }
    }
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .ok()?
        .and_hms_opt(12, 0, 0)
        .map(|v| v.and_utc())
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
}
